use anyhow::Context;
use tracing::info;

use super::move_workspace_to_monitor;
use crate::{
  traits::CommonGetters, user_config::UserConfig, wm_state::WmState,
};

/// Moves workspaces that were displaced from a disconnected monitor back
/// to it, if the monitor is connected again.
///
/// No-op unless `general.restore_workspaces_on_reconnect` is enabled.
pub fn restore_displaced_workspaces(
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  if !config.value.general.restore_workspaces_on_reconnect {
    return Ok(());
  }

  let (monitors, identities): (Vec<_>, Vec<_>) = state
    .monitors()
    .into_iter()
    .filter_map(|monitor| {
      monitor.identity().map(|identity| (monitor, identity))
    })
    .unzip();

  for workspace in state.workspaces() {
    let Some(displaced_from) = workspace.displaced_from() else {
      continue;
    };

    // Workspaces bound to a monitor via the user config are instead
    // handled by `move_bounded_workspaces_to_new_monitor`.
    if workspace.config().bind_to_monitor.is_some() {
      workspace.set_displaced_from(None);
      continue;
    }

    let Some(target_monitor) = displaced_from
      .find_match(&identities)
      .and_then(|index| monitors.get(index))
    else {
      continue;
    };

    workspace.set_displaced_from(None);

    let origin_monitor = workspace.monitor().context("No monitor.")?;

    // The workspace is already on its original monitor (e.g. when the
    // monitor it was moved to has since been reassigned to the original
    // display).
    if origin_monitor.id() == target_monitor.id() {
      continue;
    }

    info!("Restoring workspace {workspace} to monitor {target_monitor}.");

    move_workspace_to_monitor(&workspace, target_monitor, state, config)?;
  }

  Ok(())
}
