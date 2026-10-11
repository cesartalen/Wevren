use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub type PaneId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Above,
    Below,
}

impl Edge {
    pub fn at(horizontal: f32, vertical: f32) -> Option<Self> {
        if vertical < 0.25 {
            Some(Self::Above)
        } else if vertical > 0.75 {
            Some(Self::Below)
        } else if horizontal < 0.25 {
            Some(Self::Left)
        } else if horizontal > 0.75 {
            Some(Self::Right)
        } else {
            None
        }
    }
    pub fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Above | Self::Below => Axis::Vertical,
        }
    }
    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Above)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Above => "Above",
            Self::Below => "Below",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Pane {
        id: PaneId,
        path: Option<PathBuf>,
    },
    Split {
        id: usize,
        axis: Axis,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    pub fn id(&self) -> usize {
        match self {
            Self::Pane { id, .. } | Self::Split { id, .. } => *id,
        }
    }
    fn find(&self, id: usize) -> Option<&Self> {
        if self.id() == id {
            return Some(self);
        }
        match self {
            Self::Pane { .. } => None,
            Self::Split { first, second, .. } => first.find(id).or_else(|| second.find(id)),
        }
    }
    fn find_mut(&mut self, id: usize) -> Option<&mut Self> {
        if self.id() == id {
            return Some(self);
        }
        match self {
            Self::Pane { .. } => None,
            Self::Split { first, second, .. } => first.find_mut(id).or_else(|| second.find_mut(id)),
        }
    }
    fn leaves<'a>(&'a self, output: &mut Vec<(PaneId, &'a Option<PathBuf>)>) {
        match self {
            Self::Pane { id, path } => output.push((*id, path)),
            Self::Split { first, second, .. } => {
                first.leaves(output);
                second.leaves(output);
            }
        }
    }
    fn paths_mut(&mut self, update: &mut impl FnMut(&mut Option<PathBuf>)) {
        match self {
            Self::Pane { path, .. } => update(path),
            Self::Split { first, second, .. } => {
                first.paths_mut(update);
                second.paths_mut(update);
            }
        }
    }
    fn detach(self, id: PaneId) -> (Option<Self>, Option<Self>) {
        match self {
            Self::Pane { id: own, .. } if own == id => (None, Some(self)),
            Self::Pane { .. } => (Some(self), None),
            Self::Split {
                id: split,
                axis,
                ratio,
                first,
                second,
            } => {
                let (first, removed) = first.detach(id);
                if let Some(removed) = removed {
                    return (
                        Some(match first {
                            Some(first) => Self::Split {
                                id: split,
                                axis,
                                ratio,
                                first: Box::new(first),
                                second,
                            },
                            None => *second,
                        }),
                        Some(removed),
                    );
                }
                let (second, removed) = second.detach(id);
                let first = first.expect("unmatched subtree remains");
                (
                    Some(match second {
                        Some(second) => Self::Split {
                            id: split,
                            axis,
                            ratio,
                            first: Box::new(first),
                            second: Box::new(second),
                        },
                        None => first,
                    }),
                    removed,
                )
            }
        }
    }
    pub fn minimum_size(&self) -> (f32, f32) {
        match self {
            Self::Pane { .. } => (240., 160.),
            Self::Split {
                axis,
                first,
                second,
                ..
            } => {
                let a = first.minimum_size();
                let b = second.minimum_size();
                match axis {
                    Axis::Horizontal => (a.0 + b.0 + 6., a.1.max(b.1)),
                    Axis::Vertical => (a.0.max(b.0), a.1 + b.1 + 6.),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub root: Node,
    pub active: PaneId,
    #[serde(default)]
    next_id: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            root: Node::Pane { id: 0, path: None },
            active: 0,
            next_id: 1,
        }
    }
}

impl Layout {
    pub fn legacy(paths: [Option<PathBuf>; 2], split: bool, active: usize) -> Self {
        if split {
            Self {
                root: Node::Split {
                    id: 2,
                    axis: Axis::Horizontal,
                    ratio: 0.5,
                    first: Box::new(Node::Pane {
                        id: 0,
                        path: paths[0].clone(),
                    }),
                    second: Box::new(Node::Pane {
                        id: 1,
                        path: paths[1].clone(),
                    }),
                },
                active: active.min(1),
                next_id: 3,
            }
        } else {
            Self {
                root: Node::Pane {
                    id: 0,
                    path: paths[0].clone(),
                },
                active: 0,
                next_id: 1,
            }
        }
    }
    pub fn leaves(&self) -> Vec<(PaneId, &Option<PathBuf>)> {
        let mut panes = Vec::new();
        self.root.leaves(&mut panes);
        panes
    }
    pub fn projection(&self, focused: bool) -> &Node {
        if focused {
            self.root.find(self.active).unwrap_or(&self.root)
        } else {
            &self.root
        }
    }
    pub fn path(&self, id: PaneId) -> Option<&PathBuf> {
        match self.root.find(id) {
            Some(Node::Pane { path, .. }) => path.as_ref(),
            _ => None,
        }
    }
    pub fn contains(&self, id: PaneId) -> bool {
        matches!(self.root.find(id), Some(Node::Pane { .. }))
    }
    pub fn pane_for(&self, path: &Path) -> Option<PaneId> {
        self.leaves()
            .into_iter()
            .find(|(_, candidate)| candidate.as_deref() == Some(path))
            .map(|(id, _)| id)
    }
    pub fn set_path(&mut self, id: PaneId, path: Option<PathBuf>) {
        if !self.contains(id) {
            return;
        }
        if let Some(path) = &path {
            self.update_paths(|candidate| {
                if candidate.as_ref() == Some(path) {
                    *candidate = None;
                }
            });
        }
        if let Some(Node::Pane { path: current, .. }) = self.root.find_mut(id) {
            *current = path;
        }
    }
    pub fn update_paths(&mut self, mut update: impl FnMut(&mut Option<PathBuf>)) {
        self.root.paths_mut(&mut update);
    }
    fn allocate(&mut self) -> Option<usize> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        Some(id)
    }
    fn insert(&mut self, target: PaneId, node: Node, edge: Edge) -> bool {
        let Some(split_id) = self.allocate() else {
            return false;
        };
        let Some(target) = self.root.find_mut(target) else {
            return false;
        };
        if !matches!(target, Node::Pane { .. }) {
            return false;
        }
        let old = target.clone();
        let (first, second) = if edge.before() {
            (node, old)
        } else {
            (old, node)
        };
        *target = Node::Split {
            id: split_id,
            axis: edge.axis(),
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        };
        true
    }
    pub fn split(&mut self, target: PaneId, edge: Edge) -> Option<PaneId> {
        if !self.contains(target) {
            return None;
        }
        let id = self.allocate()?;
        self.insert(target, Node::Pane { id, path: None }, edge)
            .then_some(id)
    }
    pub fn close(&mut self, id: PaneId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let (root, _) = self.root.clone().detach(id);
        self.root = root.unwrap_or(Node::Pane { id, path: None });
        if !self.contains(self.active) {
            self.active = self.leaves()[0].0;
        }
        true
    }
    pub fn move_pane(&mut self, source: PaneId, target: PaneId, edge: Edge) -> bool {
        if source == target || !self.contains(source) || !self.contains(target) {
            return false;
        }
        if self.next_id == usize::MAX {
            return false;
        }
        let (root, node) = self.root.clone().detach(source);
        self.root = root.expect("moving between distinct panes leaves a target");
        self.insert(target, node.expect("validated pane exists"), edge)
    }
    pub fn resize(&mut self, id: usize, ratio: f32) {
        if ratio.is_finite()
            && let Some(Node::Split { ratio: current, .. }) = self.root.find_mut(id)
        {
            *current = ratio.clamp(0.1, 0.9);
        }
    }
    pub fn sanitize(&mut self, mut resolve: impl FnMut(&Path) -> Option<PathBuf>) {
        fn repair(
            node: Node,
            resolve: &mut impl FnMut(&Path) -> Option<PathBuf>,
            ids: &mut HashSet<usize>,
            paths: &mut HashSet<PathBuf>,
            next: &mut usize,
            depth: usize,
        ) -> Option<Node> {
            if depth > 128 {
                return None;
            }
            let original = node.id();
            let id = if original > usize::MAX / 2 || !ids.insert(original) {
                while !ids.insert(*next) {
                    *next += 1;
                }
                let id = *next;
                *next += 1;
                id
            } else {
                original
            };
            match node {
                Node::Pane { path, .. } => {
                    let path = match path {
                        Some(path) => {
                            let path = resolve(&path)?;
                            if !paths.insert(path.clone()) {
                                return None;
                            }
                            Some(path)
                        }
                        None => None,
                    };
                    Some(Node::Pane { id, path })
                }
                Node::Split {
                    axis,
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    let first = repair(*first, resolve, ids, paths, next, depth + 1);
                    let second = repair(*second, resolve, ids, paths, next, depth + 1);
                    match (first, second) {
                        (Some(first), Some(second)) => Some(Node::Split {
                            id,
                            axis,
                            ratio: if ratio.is_finite() {
                                ratio.clamp(0.1, 0.9)
                            } else {
                                0.5
                            },
                            first: Box::new(first),
                            second: Box::new(second),
                        }),
                        (first, second) => first.or(second),
                    }
                }
            }
        }
        let active_path = self
            .leaves()
            .into_iter()
            .find(|(id, _)| *id == self.active)
            .and_then(|(_, path)| path.clone());
        self.root = repair(
            self.root.clone(),
            &mut resolve,
            &mut HashSet::new(),
            &mut HashSet::new(),
            &mut 0,
            0,
        )
        .unwrap_or_else(|| Self::default().root);
        fn maximum(node: &Node) -> usize {
            match node {
                Node::Pane { id, .. } => *id,
                Node::Split {
                    id, first, second, ..
                } => (*id).max(maximum(first)).max(maximum(second)),
            }
        }
        let minimum = maximum(&self.root) + 1;
        self.next_id = if self.next_id >= usize::MAX - 2 {
            minimum
        } else {
            self.next_id.max(minimum)
        };
        if let Some(path) = active_path.and_then(|path| resolve(&path))
            && let Some(id) = self.pane_for(&path)
        {
            self.active = id;
        }
        if !self.contains(self.active) {
            self.active = self
                .leaves()
                .iter()
                .find(|(_, path)| path.is_some())
                .map_or(self.leaves()[0].0, |(id, _)| *id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_regions_match_quarter_edges_and_center() {
        assert_eq!(Edge::at(0.5, 0.1), Some(Edge::Above));
        assert_eq!(Edge::at(0.5, 0.9), Some(Edge::Below));
        assert_eq!(Edge::at(0.1, 0.5), Some(Edge::Left));
        assert_eq!(Edge::at(0.9, 0.5), Some(Edge::Right));
        assert_eq!(Edge::at(0.1, 0.1), Some(Edge::Above));
        assert_eq!(Edge::at(0.25, 0.25), None);
        assert_eq!(Edge::at(0.75, 0.75), None);
        assert_eq!(Edge::at(0.5, 0.5), None);
    }
    #[test]
    fn mixed_splits_moves_and_close_preserve_identity_and_notes() {
        let mut layout = Layout::default();
        layout.set_path(0, Some("a.md".into()));
        let right = layout.split(0, Edge::Right).unwrap();
        layout.set_path(right, Some("b.md".into()));
        let below = layout.split(right, Edge::Below).unwrap();
        layout.set_path(below, Some("c.md".into()));
        layout.active = below;
        assert_eq!(layout.leaves().len(), 3);
        assert_eq!(layout.root.minimum_size(), (486., 326.));
        assert!(layout.move_pane(below, 0, Edge::Above));
        assert_eq!(layout.active, below);
        assert_eq!(layout.path(below), Some(&PathBuf::from("c.md")));
        assert!(!layout.move_pane(below, below, Edge::Left));
        assert!(!layout.close(999));
        layout.close(right);
        assert_eq!(layout.leaves().len(), 2);
        assert_eq!(layout.active, below);
        layout.close(below);
        assert_eq!(layout.active, 0);
        layout.close(0);
        assert_eq!(layout.leaves(), vec![(0, &None)]);
    }
    #[test]
    fn closing_highest_ids_does_not_reuse_them() {
        let mut layout = Layout::default();
        let old = layout.split(0, Edge::Right).unwrap();
        let divider = layout.root.id();
        layout.close(old);
        let new = layout.split(0, Edge::Below).unwrap();
        assert!(new > old && new > divider);
        assert!(layout.root.id() > new);
        let mut restored: Layout =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        restored.sanitize(|path| Some(path.to_path_buf()));
        restored.close(new);
        assert!(restored.split(0, Edge::Left).unwrap() > new);
    }

    #[test]
    fn four_panes_focus_resize_roundtrip() {
        let mut layout = Layout::default();
        let right = layout.split(0, Edge::Right).unwrap();
        layout.split(0, Edge::Below);
        let active = layout.split(right, Edge::Below).unwrap();
        layout.active = active;
        let id = layout.root.id();
        layout.resize(id, 2.);
        layout.resize(id, f32::NAN);
        let Node::Split { ratio, .. } = layout.root else {
            panic!()
        };
        assert_eq!(ratio, 0.9);
        assert_eq!(layout.projection(true).id(), active);
        assert_eq!(layout.leaves().len(), 4);
        assert_eq!(layout.projection(false), &layout.root);
        assert_eq!(
            serde_json::from_str::<Layout>(&serde_json::to_string(&layout).unwrap()).unwrap(),
            layout
        );
    }
    #[test]
    fn sanitize_repairs_ids_prunes_missing_and_duplicate_paths() {
        let mut layout = Layout::legacy([Some("a.md".into()), Some("b.md".into())], true, 1);
        let new = layout.split(0, Edge::Below).unwrap();
        layout.set_path(new, Some("missing.md".into()));
        if let Node::Split {
            id, ratio, second, ..
        } = &mut layout.root
        {
            *id = 1;
            *ratio = f32::INFINITY;
            if let Node::Pane { id, .. } = second.as_mut() {
                *id = 1;
            }
        }
        layout.sanitize(|path| (path != Path::new("missing.md")).then(|| path.to_path_buf()));
        assert_eq!(layout.leaves().len(), 2);
        assert_eq!(layout.path(layout.active), Some(&PathBuf::from("b.md")));
        let Node::Split { ratio, .. } = layout.root else {
            panic!()
        };
        assert_eq!(ratio, 0.5);
        let mut duplicate = Layout::legacy([Some("a.md".into()), Some("a.md".into())], true, 1);
        duplicate.sanitize(|path| Some(path.to_path_buf()));
        assert_eq!(duplicate.leaves().len(), 1);
    }
}
