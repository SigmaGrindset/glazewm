use anyhow::Context;
use wm_common::try_warn;
use wm_platform::Display;

use crate::{
  commands::monitor::{
    add_monitor, mark_displaced_workspaces,
    move_bounded_workspaces_to_new_monitor, remove_monitor,
    restore_displaced_workspaces, sort_monitors, update_monitor,
  },
  models::{Monitor, MonitorIdentity, NativeMonitorProperties},
  traits::{CommonGetters, PositionGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn handle_display_settings_changed(
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  tracing::info!("Display settings changed.");

  // Ignore the event if retrieval of the displays or their properties
  // fails (can happen transiently during sleep/wake).
  let displays = try_warn!(state
    .dispatcher
    .sorted_displays()
    .map_err(anyhow::Error::from)
    .and_then(|displays| {
      displays
        .into_iter()
        .map(|display| {
          let properties = NativeMonitorProperties::try_from(&display)?;
          Ok((display, properties))
        })
        .try_collect::<Vec<_>>()
    }));

  update_monitors_from_displays(displays, state, config)
}

/// Updates the WM's monitors to match the given displays.
///
/// Monitors are added, updated, or removed as needed, and workspaces are
/// moved accordingly.
fn update_monitors_from_displays(
  displays: Vec<(Display, NativeMonitorProperties)>,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let mut pending_monitors = state.monitors();
  let mut unmatched_displays = Vec::new();

  // Match each display to an existing monitor and update it.
  for (display, properties) in displays {
    match find_matching_monitor(&pending_monitors, &properties) {
      Some((monitor, index)) => {
        update_monitor(monitor, &display, properties, state)?;
        pending_monitors.remove(index);
      }
      None => unmatched_displays.push((display, properties)),
    }
  }

  let mut new_monitors: Vec<Monitor> = Vec::new();

  // Pair unmatched displays with unmatched monitors, or add new ones.
  for (display, properties) in unmatched_displays {
    if pending_monitors.is_empty() {
      let monitor = add_monitor(display, properties, state)?;
      new_monitors.push(monitor);
    } else {
      // The monitor is reassigned to a different display (e.g. the last
      // monitor is kept when all displays are disconnected during sleep),
      // so its workspaces are no longer on their original display.
      let monitor = pending_monitors.remove(0);
      mark_displaced_workspaces(&monitor, config);
      update_monitor(&monitor, &display, properties, state)?;
    }
  }

  // Remove monitors that no longer have a corresponding display and move
  // their workspaces to other monitors.
  //
  // Prevent removal of the last monitor (i.e. for when all monitors are
  // disconnected). This will cause the WM's monitors to temporarily
  // mismatch the OS monitor state, however, it'll be updated correctly
  // when a new monitor is connected again.
  for monitor in pending_monitors {
    if state.monitors().len() > 1 {
      remove_monitor(monitor, state, config)?;
    }
  }

  // Sort monitors by position.
  sort_monitors(&state.root_container)?;

  // Restore displaced workspaces before activating workspaces on new
  // monitors, so that a new monitor only gets a fresh workspace if none
  // were restored to it.
  restore_displaced_workspaces(state, config)?;

  for new_monitor in new_monitors {
    move_bounded_workspaces_to_new_monitor(&new_monitor, state, config)?;
  }

  for window in state.windows() {
    // Display setting changes can spread windows out sporadically, so mark
    // all windows as needing a DPI adjustment (just in case).
    window.set_has_pending_dpi_adjustment(true);

    // Need to update floating position of moved windows when a monitor is
    // disconnected or if the primary display is changed. The primary
    // display dictates the position of 0,0.
    let workspace = window.workspace().context("No workspace.")?;

    let should_recenter = if window.has_custom_floating_placement() {
      let workspace_rect = workspace.to_rect()?;

      // Keep the placement if it still intersects the workspace, since
      // `PlatformEvent::DisplaySettingsChanged` can be triggered by
      // non-monitor changes (e.g. unplugging a USB device).
      window
        .floating_placement()
        .intersection_area(&workspace_rect)
        == 0
    } else {
      true
    };

    if should_recenter {
      window.set_floating_placement(
        window
          .floating_placement()
          .translate_to_center(&workspace.to_rect()?),
      );
    }
  }

  // Redraw full container tree.
  state
    .pending_sync
    .queue_container_to_redraw(state.root_container.clone());

  Ok(())
}

/// Finds the monitor matching the given display properties.
///
/// Returns the monitor and its index within the list of monitors.
///
/// # Platform-specific
///
/// - **Windows**: Additionally matches by monitor handle, which is unique
///   among connected displays, but can change over time.
fn find_matching_monitor<'a>(
  monitors: &'a [Monitor],
  properties: &NativeMonitorProperties,
) -> Option<(&'a Monitor, usize)> {
  let identity = MonitorIdentity::from_properties(properties);
  let candidates = monitors
    .iter()
    .filter_map(Monitor::identity)
    .collect::<Vec<_>>();

  monitors.iter().enumerate().find_map(|(index, monitor)| {
    #[cfg(target_os = "windows")]
    let is_handle_match =
      monitor.native_properties().handle == properties.handle;
    #[cfg(not(target_os = "windows"))]
    let is_handle_match = false;

    let is_identity_match =
      identity.as_ref().zip(monitor.identity()).is_some_and(
        |(identity, candidate)| identity.matches(&candidate, &candidates),
      );

    (is_handle_match || is_identity_match).then_some((monitor, index))
  })
}

#[cfg(test)]
mod tests {
  use anyhow::Context;
  use wm_platform::{Direction, Display, Rect};

  use super::update_monitors_from_displays;
  use crate::{
    commands::{
      container::{attach_container, set_focused_descendant},
      workspace::{activate_workspace, move_workspace_in_direction},
    },
    models::{Monitor, NativeMonitorProperties, TilingWindow},
    test_utils::{MOCK_MONITOR_HEIGHT, MOCK_MONITOR_WIDTH},
    user_config::UserConfig,
    wm_state::WmState,
  };

  const LAPTOP: &str = "AUO1234";
  const EXTERNAL: &str = "DEL40A3";
  const SECONDARY: &str = "GSM5B7F";

  /// Creates a mock display with the given device ID, positioned in the
  /// given column from left-to-right.
  fn mock_display(
    device_id: &str,
    column: u8,
  ) -> (Display, NativeMonitorProperties) {
    let bounds = Rect::from_xy(
      i32::from(column) * MOCK_MONITOR_WIDTH,
      0,
      MOCK_MONITOR_WIDTH,
      MOCK_MONITOR_HEIGHT,
    );

    #[allow(unused_mut)]
    let mut properties = NativeMonitorProperties::mock()
      .device_name(device_id.to_string())
      .device_id(device_id.to_string())
      .bounds(bounds.clone())
      .working_area(bounds)
      .call();

    #[cfg(target_os = "windows")]
    {
      properties.handle = isize::from(column) + 1;
    }

    (Display::mock(), properties)
  }

  fn laptop() -> (Display, NativeMonitorProperties) {
    mock_display(LAPTOP, 0)
  }

  fn external() -> (Display, NativeMonitorProperties) {
    mock_display(EXTERNAL, 1)
  }

  fn secondary() -> (Display, NativeMonitorProperties) {
    mock_display(SECONDARY, 2)
  }

  /// Creates a laptop with an external monitor to its right.
  ///
  /// The laptop has workspace 1, and the external monitor has workspaces
  /// 2 and 3. Each workspace has a window, and the window on workspace 1
  /// has focus.
  fn mock_state(
    restore_workspaces_on_reconnect: bool,
  ) -> anyhow::Result<(WmState, UserConfig)> {
    let (mut state, _) = WmState::mock();
    let mut config = UserConfig::mock()?;
    config.value.general.restore_workspaces_on_reconnect =
      restore_workspaces_on_reconnect;

    update_monitors_from_displays(
      vec![laptop(), external()],
      &mut state,
      &config,
    )?;

    activate_workspace(
      Some("3"),
      Some(monitor(&state, EXTERNAL)?),
      &mut state,
      &config,
    )?;

    let mut focused_window = None;

    for name in ["1", "2", "3"] {
      let workspace = state
        .workspace_by_name(name)
        .with_context(|| format!("No workspace {name}."))?;

      let window = TilingWindow::mock().call();
      attach_container(&window.clone().into(), &workspace.into(), None)?;
      focused_window.get_or_insert(window);
    }

    let focused_window = focused_window.context("No window.")?;
    set_focused_descendant(&focused_window.into(), None);

    Ok((state, config))
  }

  /// Gets the monitor for the display with the given device ID.
  fn monitor(state: &WmState, device_id: &str) -> anyhow::Result<Monitor> {
    state
      .monitors()
      .into_iter()
      .find(|monitor| monitor.native_properties().device_name == device_id)
      .with_context(|| format!("No monitor for {device_id}."))
  }

  /// Gets the names of the workspaces on the given display.
  fn workspace_names(
    state: &WmState,
    device_id: &str,
  ) -> anyhow::Result<Vec<String>> {
    Ok(
      monitor(state, device_id)?
        .workspaces()
        .iter()
        .map(|workspace| workspace.config().name)
        .collect(),
    )
  }

  /// Asserts that the workspaces are on the monitors they were on in
  /// `mock_state`.
  fn assert_initial_layout(state: &WmState) -> anyhow::Result<()> {
    assert_eq!(workspace_names(state, LAPTOP)?, ["1"]);
    assert_eq!(workspace_names(state, EXTERNAL)?, ["2", "3"]);

    for workspace in state.workspaces() {
      assert_eq!(workspace.displaced_from(), None);
    }

    Ok(())
  }

  #[test]
  fn restores_workspaces_on_reconnect() -> anyhow::Result<()> {
    let (mut state, config) = mock_state(true)?;
    assert_initial_layout(&state)?;

    update_monitors_from_displays(vec![laptop()], &mut state, &config)?;
    assert_eq!(workspace_names(&state, LAPTOP)?, ["1", "2", "3"]);

    update_monitors_from_displays(
      vec![laptop(), external()],
      &mut state,
      &config,
    )?;
    assert_initial_layout(&state)
  }

  /// Simulates all displays disconnecting during sleep, and reconnecting
  /// one at a time on wake.
  fn sleep_and_wake(
    first_display: (Display, NativeMonitorProperties),
  ) -> anyhow::Result<()> {
    let (mut state, config) = mock_state(true)?;

    update_monitors_from_displays(vec![], &mut state, &config)?;
    assert_eq!(state.monitors().len(), 1);

    update_monitors_from_displays(
      vec![first_display],
      &mut state,
      &config,
    )?;
    update_monitors_from_displays(
      vec![laptop(), external()],
      &mut state,
      &config,
    )?;

    assert_initial_layout(&state)
  }

  #[test]
  fn restores_workspaces_when_laptop_wakes_first() -> anyhow::Result<()> {
    sleep_and_wake(laptop())
  }

  #[test]
  fn restores_workspaces_when_external_wakes_first() -> anyhow::Result<()>
  {
    sleep_and_wake(external())
  }

  #[test]
  fn does_not_restore_workspaces_when_disabled() -> anyhow::Result<()> {
    let (mut state, config) = mock_state(false)?;

    update_monitors_from_displays(vec![laptop()], &mut state, &config)?;
    update_monitors_from_displays(
      vec![laptop(), external()],
      &mut state,
      &config,
    )?;

    assert_eq!(workspace_names(&state, LAPTOP)?, ["1", "2", "3"]);
    assert_eq!(workspace_names(&state, EXTERNAL)?, ["4"]);

    Ok(())
  }

  #[test]
  fn does_not_restore_explicitly_moved_workspaces() -> anyhow::Result<()> {
    let (mut state, config) = mock_state(true)?;

    update_monitors_from_displays(
      vec![laptop(), external(), secondary()],
      &mut state,
      &config,
    )?;
    update_monitors_from_displays(
      vec![laptop(), secondary()],
      &mut state,
      &config,
    )?;

    let workspace_2 =
      state.workspace_by_name("2").context("No workspace 2.")?;
    move_workspace_in_direction(
      &workspace_2,
      &Direction::Right,
      &mut state,
      &config,
    )?;

    update_monitors_from_displays(
      vec![laptop(), external(), secondary()],
      &mut state,
      &config,
    )?;

    assert_eq!(workspace_names(&state, EXTERNAL)?, ["3"]);
    assert!(workspace_names(&state, SECONDARY)?.contains(&"2".to_string()));

    Ok(())
  }
}
