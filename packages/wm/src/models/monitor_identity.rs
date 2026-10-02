use crate::models::NativeMonitorProperties;

/// Identifier of a physical monitor that persists across the monitor
/// being disconnected and reconnected.
///
/// Unlike the ID of a `Monitor` container, which is regenerated whenever
/// the container is created, this is derived from the display's hardware
/// properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorIdentity {
  #[cfg(target_os = "macos")]
  device_uuid: String,
  #[cfg(target_os = "windows")]
  device_path: Option<String>,
  #[cfg(target_os = "windows")]
  hardware_id: Option<String>,
}

impl MonitorIdentity {
  /// Gets the identity of the display with the given properties.
  ///
  /// Returns `None` if the display has no persistent identifier (e.g.
  /// virtual displays on Windows).
  pub fn from_properties(
    properties: &NativeMonitorProperties,
  ) -> Option<Self> {
    #[cfg(target_os = "macos")]
    {
      (!properties.device_uuid.is_empty()).then(|| Self {
        device_uuid: properties.device_uuid.clone(),
      })
    }
    #[cfg(target_os = "windows")]
    {
      (properties.device_path.is_some()
        || properties.hardware_id.is_some())
      .then(|| Self {
        device_path: properties.device_path.clone(),
        hardware_id: properties.hardware_id.clone(),
      })
    }
  }

  /// Whether this identity refers to the same physical monitor as
  /// `candidate`.
  ///
  /// `candidates` is the full set of identities that `candidate` is being
  /// selected from (including `candidate` itself).
  ///
  /// # Platform-specific
  ///
  /// - **Windows**: Matches by device path, falling back to the hardware
  ///   ID if it is unique within `candidates`. Device paths are unique,
  ///   but change when the monitor is connected to a different port.
  ///   Hardware IDs are shared by monitors of the same model.
  #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
  pub fn matches(&self, candidate: &Self, candidates: &[Self]) -> bool {
    #[cfg(target_os = "macos")]
    {
      self.device_uuid == candidate.device_uuid
    }
    #[cfg(target_os = "windows")]
    {
      let is_device_path_match = candidate.device_path.is_some()
        && self.device_path == candidate.device_path;

      let is_hardware_id_match =
        candidate.hardware_id.as_deref().is_some_and(|hardware_id| {
          let is_unique = candidates
            .iter()
            .filter(|other| {
              other.hardware_id.as_deref() == Some(hardware_id)
            })
            .count()
            == 1;

          is_unique && self.hardware_id.as_deref() == Some(hardware_id)
        });

      is_device_path_match || is_hardware_id_match
    }
  }

  /// Finds the first identity in `candidates` that refers to the same
  /// physical monitor as this one.
  ///
  /// Returns the index of the match within `candidates`.
  pub fn find_match(&self, candidates: &[Self]) -> Option<usize> {
    candidates
      .iter()
      .position(|candidate| self.matches(candidate, candidates))
  }
}

#[cfg(test)]
mod tests {
  use anyhow::Context;

  use super::MonitorIdentity;
  use crate::models::NativeMonitorProperties;

  /// Gets the identity of a mock display with the given device ID.
  fn identity(device_id: &str) -> anyhow::Result<MonitorIdentity> {
    MonitorIdentity::from_properties(
      &NativeMonitorProperties::mock()
        .device_id(device_id.to_string())
        .call(),
    )
    .context("Mock display has no identity.")
  }

  #[test]
  fn no_identity_without_persistent_identifier() {
    let properties = NativeMonitorProperties::mock().call();
    assert_eq!(MonitorIdentity::from_properties(&properties), None);
  }

  #[test]
  fn matches_same_device() -> anyhow::Result<()> {
    let candidates = [identity("DEL40A3")?, identity("GSM5B7F")?];

    assert_eq!(identity("GSM5B7F")?.find_match(&candidates), Some(1));
    assert_eq!(identity("AUO1234")?.find_match(&candidates), None);

    Ok(())
  }

  #[cfg(target_os = "windows")]
  mod windows {
    use super::MonitorIdentity;

    fn identity(device_path: &str, hardware_id: &str) -> MonitorIdentity {
      MonitorIdentity {
        device_path: Some(device_path.to_string()),
        hardware_id: Some(hardware_id.to_string()),
      }
    }

    #[test]
    fn matches_by_device_path() {
      let candidates = [
        identity(r"\\?\DISPLAY#DEL40A3#1", "DEL40A3"),
        identity(r"\\?\DISPLAY#DEL40A3#2", "DEL40A3"),
      ];

      assert_eq!(
        identity(r"\\?\DISPLAY#DEL40A3#2", "DEL40A3")
          .find_match(&candidates),
        Some(1)
      );
    }

    #[test]
    fn matches_by_unique_hardware_id() {
      // Same monitor connected to a different port.
      let candidates = [
        identity(r"\\?\DISPLAY#GSM5B7F#1", "GSM5B7F"),
        identity(r"\\?\DISPLAY#DEL40A3#2", "DEL40A3"),
      ];

      assert_eq!(
        identity(r"\\?\DISPLAY#DEL40A3#3", "DEL40A3")
          .find_match(&candidates),
        Some(1)
      );
    }

    #[test]
    fn ignores_ambiguous_hardware_id() {
      // Two monitors of the same model, neither on the original port.
      let candidates = [
        identity(r"\\?\DISPLAY#DEL40A3#1", "DEL40A3"),
        identity(r"\\?\DISPLAY#DEL40A3#2", "DEL40A3"),
      ];

      assert_eq!(
        identity(r"\\?\DISPLAY#DEL40A3#3", "DEL40A3")
          .find_match(&candidates),
        None
      );
    }

    #[test]
    fn ignores_missing_device_path() {
      let identity = MonitorIdentity {
        device_path: None,
        hardware_id: None,
      };

      assert_eq!(
        identity.find_match(std::slice::from_ref(&identity)),
        None
      );
    }
  }
}
