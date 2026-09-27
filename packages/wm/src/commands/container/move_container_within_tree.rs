use anyhow::Context;
use wm_common::{VecDequeExt, WmEvent};

use super::{
  attach_container, detach_container, flatten_child_split_containers,
  set_focused_descendant,
};
use crate::{models::Container, traits::CommonGetters, wm_state::WmState};

/// Move a container to a new location in the tree. This detaches the
/// container from its current parent and attaches it to the new parent at
/// the specified index.
///
/// If this container is a tiling container, its siblings are resized on
/// detach, and the container is sized to the default tiling size with its
/// new siblings. No changes to the container's tiling size are made if
/// its parent stays the same.
///
/// This will flatten any redundant split containers after moving the
/// container, which can cause the target parent to become detached. For
/// example, in the layout V[1 H[2]] where container 1 is moved down, the
/// parent gets removed resulting in V[1 2].
///
/// Emits a `WmEvent::FocusedContainerMoved` event if the container has
/// focus after the move.
pub fn move_container_within_tree(
  container_to_move: &Container,
  target_parent: &Container,
  target_index: usize,
  state: &WmState,
) -> anyhow::Result<()> {
  move_container_within_tree_without_event(
    container_to_move,
    target_parent,
    target_index,
  )?;

  if container_to_move.has_focus(None) {
    state.emit_event(WmEvent::FocusedContainerMoved {
      focused_container: container_to_move.to_dto()?,
    });
  }

  Ok(())
}

/// Same as `move_container_within_tree`, but without emitting a
/// `WmEvent::FocusedContainerMoved` event.
///
/// Useful when the move is an intermediate step of a larger change (e.g.
/// a tiling window being replaced by a non-tiling window), where the
/// container's state mid-change shouldn't be broadcast.
pub fn move_container_within_tree_without_event(
  container_to_move: &Container,
  target_parent: &Container,
  target_index: usize,
) -> anyhow::Result<()> {
  // Create iterator of parent, grandparent, and great-grandparent.
  let ancestors =
    container_to_move.ancestors().take(3).collect::<Vec<_>>();

  // Get lowest common ancestor (LCA) between `container_to_move` and
  // `target_parent`. This could be the `target_parent` itself.
  let lowest_common_ancestor =
    lowest_common_ancestor(container_to_move, target_parent)
      .context("No common ancestor between containers.")?;

  // If the container is already a child of the target parent, then shift
  // it to the target index.
  if container_to_move.parent().context("No parent.")? == *target_parent {
    target_parent
      .borrow_children_mut()
      .shift_to_index(target_index, container_to_move.clone());

    return Ok(());
  }

  // Handle case where target parent is the LCA. For example, when swapping
  // sibling containers or moving a container to a direct ancestor.
  if *target_parent == lowest_common_ancestor {
    return move_to_lowest_common_ancestor(
      container_to_move,
      &lowest_common_ancestor,
      target_index,
    );
  }

  // Get ancestor of `container_to_move` that is a direct child of the LCA.
  // This could be the `container_to_move` itself.
  let container_to_move_ancestor = container_to_move
    .self_and_ancestors()
    .find(|ancestor| {
      ancestor.parent() == Some(lowest_common_ancestor.clone())
    })
    .context("Failed to get ancestor of container to move.")?;

  // Likewise, get ancestor of `target_parent` that is a direct child of
  // the LCA.
  let target_parent_ancestor = target_parent
    .self_and_ancestors()
    .find(|ancestor| {
      ancestor.parent() == Some(lowest_common_ancestor.clone())
    })
    .context("Failed to get ancestor of target parent.")?;

  // Get whether the container is the focused descendant in its original
  // subtree from the LCA.
  let is_focused_descendant = *container_to_move
    == container_to_move_ancestor
    || container_to_move
      .has_focus(Some(container_to_move_ancestor.clone()));

  // Get whether the ancestor of `container_to_move` appears before
  // `target_parent`'s ancestor in the child focus order of the LCA.
  let original_focus_index = container_to_move_ancestor.focus_index();
  let is_subtree_focused =
    original_focus_index < target_parent_ancestor.focus_index();

  detach_container(container_to_move.clone())?;
  attach_container(
    &container_to_move.clone(),
    &target_parent.clone(),
    Some(target_index),
  )?;

  // Set `container_to_move` as focused descendant within target subtree if
  // its original subtree had focus more recently (even if the container is
  // not the last focused within that subtree).
  if is_subtree_focused {
    set_focused_descendant(
      container_to_move,
      Some(&target_parent_ancestor),
    );
  }

  // If the focused descendant is moved to the targets subtree, then the
  // target's ancestor should be placed before the original ancestor in
  // LCA's child focus order.
  if is_focused_descendant && is_subtree_focused {
    lowest_common_ancestor
      .borrow_child_focus_order_mut()
      .shift_to_index(original_focus_index, target_parent_ancestor.id());
  }

  // After moving the container, flatten any redundant split containers.
  // For example, in the layout V[1 H[2]] where container 1 is moved down
  // to become V[H[1 2]], this will then need to be flattened to V[1 2].
  for ancestor in ancestors.iter().rev() {
    flatten_child_split_containers(ancestor)?;
  }

  Ok(())
}

fn move_to_lowest_common_ancestor(
  container_to_move: &Container,
  lowest_common_ancestor: &Container,
  target_index: usize,
) -> anyhow::Result<()> {
  // Keep reference to focus index of container's ancestor in LCA's child
  // focus order.
  let original_focus_index = container_to_move
    .self_and_ancestors()
    .find(|ancestor| {
      ancestor.parent() == Some(lowest_common_ancestor.clone())
    })
    .map(|ancestor| ancestor.focus_index())
    .context("Failed to get focus index of container's ancestor.")?;

  detach_container(container_to_move.clone())?;

  attach_container(
    &container_to_move.clone(),
    &lowest_common_ancestor.clone(),
    Some(target_index),
  )?;

  lowest_common_ancestor
    .borrow_child_focus_order_mut()
    .shift_to_index(original_focus_index, container_to_move.id());

  Ok(())
}

/// Gets the lowest container in the tree that has both `container_a` and
/// `container_b` as descendants.
pub fn lowest_common_ancestor(
  container_a: &Container,
  container_b: &Container,
) -> Option<Container> {
  let mut ancestor_a = Some(container_a.clone());

  // Traverse upwards from container A.
  while let Some(current_ancestor_a) = ancestor_a {
    let mut ancestor_b = Some(container_b.clone());

    // Traverse upwards from container B.
    while let Some(current_ancestor_b) = ancestor_b {
      if current_ancestor_a == current_ancestor_b {
        return Some(current_ancestor_a);
      }

      ancestor_b = current_ancestor_b.parent();
    }

    ancestor_a = current_ancestor_a.parent();
  }

  None
}

#[cfg(test)]
mod tests {
  use tokio::sync::mpsc;
  use wm_common::{ContainerDto, TilingDirection, WmEvent};

  use super::{
    move_container_within_tree, move_container_within_tree_without_event,
  };
  use crate::{
    commands::container::set_focused_descendant,
    models::{Monitor, SplitContainer, TilingWindow, Workspace},
    traits::CommonGetters,
    wm_state::WmState,
  };

  /// Creates the layout H[1 V[2]], where window 2 has focus.
  ///
  /// Returns the workspace and window 2.
  fn mock_layout() -> (Workspace, TilingWindow) {
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();

    let split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![window_b.clone().into()])
      .call();

    let workspace = Workspace::mock()
      .tiling_containers(vec![window_a.into(), split.into()])
      .call();

    // Monitor is needed for resolving the window positions in DTOs.
    let _monitor =
      Monitor::mock().workspaces(vec![workspace.clone()]).call();

    set_focused_descendant(&window_b.clone().into(), None);

    (workspace, window_b)
  }

  /// Drains all events emitted so far.
  fn drain_events(
    event_rx: &mut mpsc::UnboundedReceiver<WmEvent>,
  ) -> Vec<WmEvent> {
    std::iter::from_fn(|| event_rx.try_recv().ok()).collect()
  }

  #[test]
  fn emits_event_when_moving_focused_container() -> anyhow::Result<()> {
    let (state, mut event_rx) = WmState::mock();
    let (workspace, window) = mock_layout();

    move_container_within_tree(
      &window.clone().into(),
      &workspace.clone().into(),
      workspace.child_count(),
      &state,
    )?;

    assert!(matches!(
      drain_events(&mut event_rx).as_slice(),
      [WmEvent::FocusedContainerMoved {
        focused_container: ContainerDto::Window(dto),
      }] if dto.id == window.id()
        && dto.parent_id == Some(workspace.id())
    ));

    Ok(())
  }

  #[test]
  fn does_not_emit_event_when_moving_without_event() -> anyhow::Result<()>
  {
    let (_state, mut event_rx) = WmState::mock();
    let (workspace, window) = mock_layout();

    move_container_within_tree_without_event(
      &window.clone().into(),
      &workspace.clone().into(),
      workspace.child_count(),
    )?;

    assert_eq!(window.parent(), Some(workspace.into()));
    assert!(drain_events(&mut event_rx).is_empty());

    Ok(())
  }
}
