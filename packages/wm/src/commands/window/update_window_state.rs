use anyhow::Context;
use tracing::{info, warn};
use wm_common::{WindowState, WmEvent};

use crate::{
  commands::container::{
    move_container_within_tree, move_container_within_tree_without_event,
    replace_container, resize_tiling_container,
  },
  models::{Container, InsertionTarget, WindowContainer},
  traits::{CommonGetters, TilingSizeGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};

/// Updates the state of a window.
///
/// Adds the window for redraw if there is a state change.
///
/// Returns the window after the state change.
pub fn update_window_state(
  window: WindowContainer,
  target_state: WindowState,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  if window.state() == target_state {
    return Ok(window);
  }

  info!("Updating window state: {:?}.", target_state);

  match target_state {
    WindowState::Tiling => set_tiling(&window, state, config),
    _ => set_non_tiling(window, target_state, state),
  }
}

/// Updates the state of a window to be `WindowState::Tiling`.
fn set_tiling(
  window: &WindowContainer,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<WindowContainer> {
  let window = window
    .as_non_tiling_window()
    .context("Invalid window state.")?
    .clone();

  let workspace =
    window.workspace().context("Window has no workspace.")?;

  // Check whether insertion target is still valid.
  let insertion_target =
    window.insertion_target().filter(|insertion_target| {
      insertion_target
        .target_parent
        .workspace()
        .is_some_and(|workspace| workspace.is_displayed())
    });

  // Get the position in the tree to insert the new tiling window. This
  // will be the window's previous tiling position if it has one, or
  // instead beside the last focused tiling window in the workspace.
  let (target_parent, target_index) = insertion_target
    .as_ref()
    .map(|insertion_target| {
      (
        insertion_target.target_parent.clone(),
        insertion_target.target_index,
      )
    })
    // Fallback to the last focused tiling window within the workspace.
    .or_else(|| {
      let focused_window = workspace
        .descendant_focus_order()
        .find(Container::is_tiling_window)?;

      Some((focused_window.parent()?, focused_window.index() + 1))
    })
    // Default to inserting at the end of the workspace.
    .unwrap_or((workspace.clone().into(), workspace.child_count()));

  let tiling_window = window.to_tiling(config.value.gaps.clone());

  // Replace the original window with the created tiling window.
  replace_container(
    &tiling_window.clone().into(),
    &window.parent().context("No parent.")?,
    window.index(),
  )?;

  move_container_within_tree(
    &tiling_window.clone().into(),
    &target_parent,
    target_index,
    state,
  )?;

  #[allow(clippy::cast_precision_loss)]
  if let Some(insertion_target) = &insertion_target {
    let size_scale = (insertion_target.prev_sibling_count + 1) as f32
      / (tiling_window.tiling_siblings().count() + 1) as f32;

    // Scale the window's previous size based on the current number of
    // siblings. E.g. if the window was 0.5 with 1 sibling, and now has 2
    // siblings, scale to 0.5 * (2/3) to maintain proportional sizing.
    let target_size = insertion_target.prev_tiling_size * size_scale;
    resize_tiling_container(&tiling_window.clone().into(), target_size);
  }

  state
    .pending_sync
    .queue_containers_to_redraw(target_parent.tiling_children())
    .queue_workspace_to_reorder(workspace);

  Ok(tiling_window.into())
}

/// Updates the state of a window to be either `WindowState::Floating`,
/// `WindowState::Fullscreen`, or `WindowState::Minimized`.
fn set_non_tiling(
  window: WindowContainer,
  target_state: WindowState,
  state: &mut WmState,
) -> anyhow::Result<WindowContainer> {
  // A window can only be updated to a minimized state if it is
  // natively minimized.
  // TODO: Consider doing the same for maximized and fullscreen states.
  if target_state == WindowState::Minimized
    && !window.native_properties().is_minimized
  {
    info!("No window state update. Minimizing window.");

    // TODO: Instead of doing the platform call directly here, instead add
    // a `queue_state_change` method to `PendingSync`.
    if let Err(err) = window.native().minimize() {
      warn!("Failed to minimize window: {}", err);
    }

    return Ok(window);
  }

  let workspace = window.workspace().context("No workspace.")?;

  match window {
    WindowContainer::NonTilingWindow(window) => {
      let current_state = window.state();

      // Update the window's previous state if the discriminant changes.
      // TODO: Move out handling of active drag. Can then simplify calls to
      // `set_active_drag` in `handle_window_moved_or_resized_end`.
      if !current_state.is_same_state(&target_state)
        && window.active_drag().is_none()
      {
        window.set_prev_state(current_state);
        state.pending_sync.queue_workspace_to_reorder(workspace);
      }

      window.set_state(target_state);
      state.pending_sync.queue_container_to_redraw(window.clone());

      Ok(window.into())
    }
    WindowContainer::TilingWindow(window) => {
      let parent = window.parent().context("No parent")?;
      let is_workspace_child = parent == workspace.clone().into();

      let non_tiling_window = window.to_non_tiling(
        target_state.clone(),
        Some(InsertionTarget {
          target_parent: parent.clone(),
          target_index: window.index(),
          prev_tiling_size: window.tiling_size(),
          prev_sibling_count: window.tiling_siblings().count(),
        }),
      );

      // Non-tiling windows should always be direct children of the
      // workspace. The window is still tiling at this point, so the move
      // event is instead emitted once it has been replaced.
      if !is_workspace_child {
        move_container_within_tree_without_event(
          &window.clone().into(),
          &workspace.clone().into(),
          workspace.child_count(),
        )?;
      }

      replace_container(
        &non_tiling_window.clone().into(),
        &workspace.clone().into(),
        window.index(),
      )?;

      if !is_workspace_child && non_tiling_window.has_focus(None) {
        state.emit_event(WmEvent::FocusedContainerMoved {
          focused_container: non_tiling_window.to_dto()?,
        });
      }

      state
        .pending_sync
        .queue_container_to_redraw(non_tiling_window.clone())
        .queue_containers_to_redraw(workspace.tiling_children())
        .queue_workspace_to_reorder(workspace);

      Ok(non_tiling_window.into())
    }
  }
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_common::{
    ContainerDto, FloatingStateConfig, TilingDirection, WindowState,
    WmEvent,
  };

  use super::set_non_tiling;
  use crate::{
    commands::container::set_focused_descendant,
    models::{
      Monitor, SplitContainer, TilingContainer, TilingWindow, Workspace,
    },
    traits::CommonGetters,
    wm_state::WmState,
  };

  /// Creates a workspace containing the given tiling containers, and
  /// focuses the given window.
  fn mock_workspace(
    tiling_containers: Vec<TilingContainer>,
    focused_window: &TilingWindow,
  ) -> Workspace {
    let workspace = Workspace::mock()
      .tiling_containers(tiling_containers)
      .call();

    // Monitor is needed for resolving the window positions in DTOs.
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    set_focused_descendant(&focused_window.clone().into(), None);

    workspace
  }

  /// Drains all events emitted so far.
  fn drain_events(
    event_rx: &mut mpsc::UnboundedReceiver<WmEvent>,
  ) -> Vec<WmEvent> {
    std::iter::from_fn(|| event_rx.try_recv().ok()).collect()
  }

  #[test]
  fn emits_only_final_state_when_leaving_split() -> anyhow::Result<()> {
    let (mut state, mut event_rx) = WmState::mock();

    // Layout of H[1 V[2]], where window 2 has focus.
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();

    let split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![window_b.clone().into()])
      .call();

    let workspace =
      mock_workspace(vec![window_a.into(), split.into()], &window_b);

    let window = set_non_tiling(
      window_b.into(),
      WindowState::Floating(FloatingStateConfig::default()),
      &mut state,
    )?;

    assert_eq!(window.parent(), Some(workspace.into()));

    // The intermediate tiling state of the window (after being moved out
    // of the split container) should not be broadcast.
    assert!(matches!(
      drain_events(&mut event_rx).as_slice(),
      [WmEvent::FocusedContainerMoved {
        focused_container: ContainerDto::Window(dto),
      }] if dto.id == window.id()
        && matches!(dto.state, WindowState::Floating(_))
    ));

    Ok(())
  }

  #[test]
  fn emits_no_event_when_leaving_workspace() -> anyhow::Result<()> {
    let (mut state, mut event_rx) = WmState::mock();

    // Layout of H[1 2], where window 2 has focus.
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();

    mock_workspace(
      vec![window_a.into(), window_b.clone().into()],
      &window_b,
    );

    set_non_tiling(
      window_b.into(),
      WindowState::Floating(FloatingStateConfig::default()),
      &mut state,
    )?;

    assert!(drain_events(&mut event_rx).is_empty());

    Ok(())
  }
}
