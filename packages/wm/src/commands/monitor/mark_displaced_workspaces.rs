use crate::{models::Monitor, user_config::UserConfig};

/// Records the monitor as the origin of each of its workspaces, so that
/// they can be moved back via `restore_displaced_workspaces` once the
/// monitor is reconnected.
///
/// Should be called before the monitor is removed or reassigned to a
/// different display.
///
/// Workspaces that are bound to a monitor via the user config, or that
/// already have a recorded origin (e.g. from an earlier disconnect of
/// another monitor), are left unchanged.
///
/// No-op unless `general.restore_workspaces_on_reconnect` is enabled.
pub fn mark_displaced_workspaces(monitor: &Monitor, config: &UserConfig) {
  if !config.value.general.restore_workspaces_on_reconnect {
    return;
  }

  let Some(identity) = monitor.identity() else {
    return;
  };

  let workspaces = monitor.workspaces().into_iter().filter(|workspace| {
    workspace.config().bind_to_monitor.is_none()
      && workspace.displaced_from().is_none()
  });

  for workspace in workspaces {
    workspace.set_displaced_from(Some(identity.clone()));
  }
}
