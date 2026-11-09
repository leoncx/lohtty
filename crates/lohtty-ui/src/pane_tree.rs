//! The split layout of a tab: a tree whose leaves are panes.
//!
//! Generic over the pane type so the logic is testable without gpui.

/// Identifies a split node, stable across edits (used for divider dragging).
pub type SplitId = u64;

/// How a split lays out its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Side by side (a vertical divider).
    Horizontal,
    /// Stacked (a horizontal divider).
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone)]
pub enum Node<T> {
    Leaf(T),
    Split(Split<T>),
}

#[derive(Debug, Clone)]
pub struct Split<T> {
    pub id: SplitId,
    pub axis: Axis,
    pub children: Vec<Node<T>>,
    /// Fraction of the split's size given to each child; sums to 1.
    pub ratios: Vec<f32>,
}

/// Result of removing a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    Yes,
    /// The pane is the only one left: the caller should close the tab.
    LastPane,
    NotFound,
}

/// Smallest fraction a pane can be resized to.
const MIN_RATIO: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug, Clone)]
pub struct PaneTree<T> {
    root: Node<T>,
    next_id: SplitId,
}

impl<T: Clone + PartialEq> PaneTree<T> {
    pub fn new(pane: T) -> Self {
        Self {
            root: Node::Leaf(pane),
            next_id: 0,
        }
    }

    pub fn root(&self) -> &Node<T> {
        &self.root
    }

    /// All panes, in layout order (left to right, top to bottom).
    pub fn panes(&self) -> Vec<T> {
        fn walk<T: Clone>(node: &Node<T>, out: &mut Vec<T>) {
            match node {
                Node::Leaf(t) => out.push(t.clone()),
                Node::Split(s) => s.children.iter().for_each(|c| walk(c, out)),
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn contains(&self, pane: &T) -> bool {
        self.panes().contains(pane)
    }

    /// Splits `target`, placing `new` after it (right of or below it).
    /// Returns false if `target` isn't in the tree.
    pub fn split(&mut self, target: &T, axis: Axis, new: T) -> bool {
        let mut new = Some(new);
        let mut next_id = self.next_id;
        let done = split_in(&mut self.root, target, axis, &mut new, &mut next_id);
        self.next_id = next_id;
        done
    }

    pub fn remove(&mut self, target: &T) -> Removed {
        match &self.root {
            Node::Leaf(t) if t == target => Removed::LastPane,
            Node::Leaf(_) => Removed::NotFound,
            Node::Split(_) => {
                if remove_in(&mut self.root, target) {
                    Removed::Yes
                } else {
                    Removed::NotFound
                }
            }
        }
    }

    /// Moves the divider after child `index` of split `id` to `position`
    /// (0..1 along the split's axis), keeping every pane at least 5% large.
    pub fn resize(&mut self, id: SplitId, index: usize, position: f32) {
        let Some(split) = find_split(&mut self.root, id) else {
            return;
        };
        if index + 1 >= split.ratios.len() {
            return;
        }
        let start: f32 = split.ratios[..index].iter().sum();
        let pair = split.ratios[index] + split.ratios[index + 1];
        let first = (position - start).clamp(
            MIN_RATIO.min(pair / 2.0),
            (pair - MIN_RATIO).max(pair / 2.0),
        );
        split.ratios[index] = first;
        split.ratios[index + 1] = pair - first;
    }

    /// The axis of split `id`, if it exists.
    pub fn axis_of(&self, id: SplitId) -> Option<Axis> {
        fn walk<T>(node: &Node<T>, id: SplitId) -> Option<Axis> {
            let Node::Split(s) = node else { return None };
            if s.id == id {
                return Some(s.axis);
            }
            s.children.iter().find_map(|c| walk(c, id))
        }
        walk(&self.root, id)
    }

    /// The pane adjacent to `from` in `direction`, if any.
    pub fn neighbor(&self, from: &T, direction: Direction) -> Option<T> {
        let layout = self.layout();
        let (_, f) = layout.iter().find(|(t, _)| t == from)?;
        const EPS: f32 = 1e-4;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);

        layout
            .iter()
            .filter(|(t, _)| t != from)
            .filter_map(|(t, r)| {
                let (gap, shared) = match direction {
                    Direction::Right => {
                        (r.x - (f.x + f.w), overlap(f.y, f.y + f.h, r.y, r.y + r.h))
                    }
                    Direction::Left => (f.x - (r.x + r.w), overlap(f.y, f.y + f.h, r.y, r.y + r.h)),
                    Direction::Down => (r.y - (f.y + f.h), overlap(f.x, f.x + f.w, r.x, r.x + r.w)),
                    Direction::Up => (f.y - (r.y + r.h), overlap(f.x, f.x + f.w, r.x, r.x + r.w)),
                };
                (gap >= -EPS && shared > EPS).then_some((t, gap, shared))
            })
            // Nearest first, then the one sharing the longest edge.
            .min_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
            })
            .map(|(t, _, _)| t.clone())
    }

    /// Each pane's rectangle within the unit square.
    fn layout(&self) -> Vec<(T, Rect)> {
        fn walk<T: Clone>(node: &Node<T>, r: Rect, out: &mut Vec<(T, Rect)>) {
            match node {
                Node::Leaf(t) => out.push((t.clone(), r)),
                Node::Split(s) => {
                    let mut offset = 0.0;
                    for (child, ratio) in s.children.iter().zip(&s.ratios) {
                        let cr = match s.axis {
                            Axis::Horizontal => Rect {
                                x: r.x + r.w * offset,
                                w: r.w * ratio,
                                ..r
                            },
                            Axis::Vertical => Rect {
                                y: r.y + r.h * offset,
                                h: r.h * ratio,
                                ..r
                            },
                        };
                        walk(child, cr, out);
                        offset += ratio;
                    }
                }
            }
        }
        let mut out = Vec::new();
        let unit = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        walk(&self.root, unit, &mut out);
        out
    }
}

fn split_in<T: PartialEq>(
    node: &mut Node<T>,
    target: &T,
    axis: Axis,
    new: &mut Option<T>,
    next_id: &mut SplitId,
) -> bool {
    match node {
        Node::Leaf(t) if t == target => {
            let Some(new) = new.take() else { return false };
            let split = Node::Split(Split {
                id: *next_id,
                axis,
                children: Vec::new(),
                ratios: vec![0.5, 0.5],
            });
            *next_id += 1;
            let old = std::mem::replace(node, split);
            if let Node::Split(s) = node {
                s.children = vec![old, Node::Leaf(new)];
            }
            true
        }
        Node::Leaf(_) => false,
        Node::Split(s) => {
            // Same direction as this split: add a sibling instead of nesting.
            if s.axis == axis
                && let Some(i) = s
                    .children
                    .iter()
                    .position(|c| matches!(c, Node::Leaf(t) if t == target))
                && let Some(new) = new.take()
            {
                let half = s.ratios[i] / 2.0;
                s.ratios[i] = half;
                s.children.insert(i + 1, Node::Leaf(new));
                s.ratios.insert(i + 1, half);
                return true;
            }
            s.children
                .iter_mut()
                .any(|c| split_in(c, target, axis, new, next_id))
        }
    }
}

fn remove_in<T: PartialEq>(node: &mut Node<T>, target: &T) -> bool {
    let Node::Split(s) = node else { return false };
    let Some(i) = s
        .children
        .iter()
        .position(|c| matches!(c, Node::Leaf(t) if t == target))
    else {
        return s.children.iter_mut().any(|c| remove_in(c, target));
    };
    s.children.remove(i);
    s.ratios.remove(i);
    let total: f32 = s.ratios.iter().sum();
    s.ratios.iter_mut().for_each(|r| *r /= total);
    // A split with a single child is replaced by that child.
    let only = (s.children.len() == 1).then(|| s.children.pop().expect("one child left"));
    if let Some(only) = only {
        *node = only;
    }
    true
}

fn find_split<T>(node: &mut Node<T>, id: SplitId) -> Option<&mut Split<T>> {
    let Node::Split(s) = node else { return None };
    if s.id == id {
        return Some(s);
    }
    s.children.iter_mut().find_map(|c| find_split(c, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratios(tree: &PaneTree<u32>) -> Vec<f32> {
        match tree.root() {
            Node::Split(s) => s.ratios.clone(),
            Node::Leaf(_) => vec![1.0],
        }
    }

    #[test]
    fn split_creates_a_split_node() {
        let mut t = PaneTree::new(1);
        assert!(t.split(&1, Axis::Horizontal, 2));
        assert_eq!(t.panes(), vec![1, 2]);
        assert_eq!(ratios(&t), vec![0.5, 0.5]);
        assert!(!t.split(&9, Axis::Horizontal, 3));
    }

    #[test]
    fn same_axis_split_adds_a_sibling() {
        let mut t = PaneTree::new(1);
        t.split(&1, Axis::Horizontal, 2);
        t.split(&1, Axis::Horizontal, 3);
        assert_eq!(t.panes(), vec![1, 3, 2]);
        assert_eq!(ratios(&t), vec![0.25, 0.25, 0.5]);
    }

    #[test]
    fn cross_axis_split_nests() {
        let mut t = PaneTree::new(1);
        t.split(&1, Axis::Horizontal, 2);
        t.split(&2, Axis::Vertical, 3);
        let Node::Split(root) = t.root() else {
            panic!()
        };
        assert_eq!(root.axis, Axis::Horizontal);
        assert!(matches!(&root.children[1], Node::Split(s) if s.axis == Axis::Vertical));
        assert_eq!(t.panes(), vec![1, 2, 3]);
    }

    #[test]
    fn remove_collapses_and_rebalances() {
        let mut t = PaneTree::new(1);
        t.split(&1, Axis::Horizontal, 2);
        t.split(&2, Axis::Vertical, 3);
        assert_eq!(t.remove(&3), Removed::Yes);
        assert_eq!(t.panes(), vec![1, 2]);
        assert_eq!(t.remove(&1), Removed::Yes);
        assert!(matches!(t.root(), Node::Leaf(2)));
        assert_eq!(t.remove(&2), Removed::LastPane);
        assert_eq!(t.remove(&7), Removed::NotFound);
    }

    #[test]
    fn remove_redistributes_ratios() {
        let mut t = PaneTree::new(1);
        t.split(&1, Axis::Horizontal, 2);
        t.split(&2, Axis::Horizontal, 3);
        assert_eq!(ratios(&t), vec![0.5, 0.25, 0.25]);
        t.remove(&1);
        assert_eq!(ratios(&t), vec![0.5, 0.5]);
    }

    #[test]
    fn neighbors_follow_geometry() {
        // 1 | 2
        //   | -
        //   | 3
        let mut t = PaneTree::new(1);
        t.split(&1, Axis::Horizontal, 2);
        t.split(&2, Axis::Vertical, 3);
        assert_eq!(t.neighbor(&1, Direction::Right), Some(2));
        assert_eq!(t.neighbor(&3, Direction::Left), Some(1));
        assert_eq!(t.neighbor(&2, Direction::Down), Some(3));
        assert_eq!(t.neighbor(&3, Direction::Up), Some(2));
        assert_eq!(t.neighbor(&1, Direction::Left), None);
        assert_eq!(t.neighbor(&2, Direction::Up), None);
    }

    #[test]
    fn resize_moves_divider_with_limits() {
        let mut t = PaneTree::new(1);
        t.split(&1, Axis::Horizontal, 2);
        let Node::Split(s) = t.root() else { panic!() };
        let id = s.id;
        t.resize(id, 0, 0.7);
        let r = ratios(&t);
        assert!((r[0] - 0.7).abs() < 1e-6 && (r[1] - 0.3).abs() < 1e-6);
        t.resize(id, 0, 1.0);
        assert!((ratios(&t)[1] - MIN_RATIO).abs() < 1e-6);
        t.resize(id, 5, 0.5); // out of range: ignored
    }
}
