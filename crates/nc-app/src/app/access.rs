//! The accessibility tree (AccessKit → AT-SPI on Linux), rebuilt from the shell's regions whenever the
//! chrome is: every control with its label and role, keyboard focus as the shell tracks it, and the
//! canvas as a node whose label summarises the sketch (UX principle 9).

use super::App;
use accesskit::{
    Action as AxAction, Node, NodeId, Rect as AxRect, Role as AxRole, Toggled, TreeId, TreeInfo,
    TreeUpdate,
};
use nc_shell::{Hit, Role};

const ROOT: NodeId = NodeId(1);
const CANVAS: NodeId = NodeId(2);
/// Region `i` of the current layout is node `FIRST_REGION + i`.
const FIRST_REGION: u64 = 16;

impl App {
    fn access_tree(&self) -> TreeUpdate {
        let scale = self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor());
        let size = self.logical_size();
        let bounds = |b: [f32; 4]| AxRect {
            x0: b[0] as f64 * scale,
            y0: b[1] as f64 * scale,
            x1: (b[0] + b[2]) as f64 * scale,
            y1: (b[1] + b[3]) as f64 * scale,
        };
        let mut nodes = Vec::new();
        let mut children = vec![CANVAS];
        let mut canvas = Node::new(AxRole::Canvas);
        canvas.set_label(self.summary());
        canvas.set_bounds(bounds([0.0, 0.0, size.x, size.y]));
        canvas.add_action(AxAction::Focus);
        nodes.push((CANVAS, canvas));
        let mut focus = CANVAS;
        for (i, r) in self.shell.regions().iter().enumerate() {
            if r.label.is_empty() || (r.hit == Hit::Panel && r.role == Role::Group && !r.focusable)
            {
                continue;
            }
            let id = NodeId(FIRST_REGION + i as u64);
            let role = match r.role {
                Role::Button => AxRole::Button,
                // A button with a toggled state is how AT-SPI reads a toggle button.
                Role::Toggle { .. } => AxRole::Button,
                Role::TextField => AxRole::TextInput,
                Role::ListItem { .. } => AxRole::ListItem,
                Role::Label => AxRole::Label,
                Role::Group => AxRole::Group,
            };
            let mut node = Node::new(role);
            node.set_label(r.label.clone());
            node.set_bounds(bounds(r.bounds));
            match r.role {
                Role::Toggle { on } => {
                    node.set_toggled(if on { Toggled::True } else { Toggled::False })
                }
                Role::ListItem { selected } => node.set_selected(selected),
                // AT-SPI reads a label's and a text field's text from its value.
                Role::TextField | Role::Label => node.set_value(r.label.clone()),
                _ => {}
            }
            if r.focusable {
                node.add_action(AxAction::Click);
                node.add_action(AxAction::Focus);
            }
            if self.shell.focus == Some(r.hit) && r.focusable {
                focus = id;
            }
            nodes.push((id, node));
            children.push(id);
        }
        let mut root = Node::new(AxRole::Window);
        root.set_label(format!(
            "{} · {}",
            self.sketch_name(),
            nc_shell::copy::APP_NAME
        ));
        root.set_children(children);
        nodes.push((ROOT, root));
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus,
        }
    }

    /// Sends the current tree, if a screen reader is listening.
    pub(super) fn update_access(&mut self) {
        let Some(mut adapter) = self.access.take() else {
            return;
        };
        adapter.update_if_active(|| self.access_tree());
        self.access = Some(adapter);
    }

    pub(super) fn access_event(&mut self, event: accesskit_winit::WindowEvent) {
        match event {
            accesskit_winit::WindowEvent::InitialTreeRequested => self.update_access(),
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                let region = request
                    .target_node
                    .0
                    .checked_sub(FIRST_REGION)
                    .and_then(|i| self.shell.regions().get(i as usize))
                    .map(|r| r.hit);
                match (request.action, region) {
                    (AxAction::Click, Some(hit)) => self.activate(hit),
                    (AxAction::Focus, Some(hit)) => {
                        self.shell.focus = Some(hit);
                        self.redraw();
                    }
                    (AxAction::Focus, None) => {
                        self.shell.focus = None;
                        self.redraw();
                    }
                    _ => {}
                }
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }
}
