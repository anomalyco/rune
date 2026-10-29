//! Small-vector child storage that holds up to two `NodeId`s inline without heap allocation.

use taffy::NodeId;

/// Child list for a layout node: 0, 1, or 2 children inline (24 bytes total), spilling to an
/// exact-sized boxed slice for 3+ children.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SmallChildren {
    #[default]
    Empty,
    One(NodeId),
    Two([NodeId; 2]),
    Heap(Box<[NodeId]>),
}

impl SmallChildren {
    #[inline(always)]
    pub fn from_slice(children: &[NodeId]) -> Self {
        match children {
            [] => Self::Empty,
            [a] => Self::One(*a),
            [a, b] => Self::Two([*a, *b]),
            many => Self::Heap(many.into()),
        }
    }

    #[inline(always)]
    pub fn set_from_slice(&mut self, children: &[NodeId]) {
        match children {
            [] => *self = Self::Empty,
            [a] => *self = Self::One(*a),
            [a, b] => *self = Self::Two([*a, *b]),
            many => {
                if let Self::Heap(existing) = self
                    && existing.len() == many.len()
                {
                    existing.copy_from_slice(many);
                    return;
                }
                *self = Self::Heap(many.into());
            }
        }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[NodeId] {
        match self {
            Self::Empty => &[],
            Self::One(a) => core::slice::from_ref(a),
            Self::Two(pair) => pair.as_slice(),
            Self::Heap(slice) => slice,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_) => 2,
            Self::Heap(slice) => slice.len(),
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    #[inline(always)]
    pub fn get(&self, index: usize) -> NodeId {
        self.as_slice()[index]
    }

    #[inline(always)]
    pub fn iter(&self) -> SmallChildrenIter<'_> {
        SmallChildrenIter(self.as_slice().iter())
    }

    pub fn push(&mut self, child: NodeId) {
        match *self {
            Self::Empty => *self = Self::One(child),
            Self::One(a) => *self = Self::Two([a, child]),
            Self::Two([a, b]) => *self = Self::Heap(Box::new([a, b, child])),
            Self::Heap(ref mut slice) => {
                let mut vec = Vec::with_capacity(slice.len() + 1);
                vec.extend_from_slice(slice);
                vec.push(child);
                *self = Self::Heap(vec.into_boxed_slice());
            }
        }
    }

    pub fn insert(&mut self, index: usize, child: NodeId) {
        let mut vec = self.as_slice().to_vec();
        vec.insert(index, child);
        self.set_from_slice(&vec);
    }

    pub fn remove(&mut self, index: usize) -> NodeId {
        match *self {
            Self::Empty => panic!("remove on empty SmallChildren"),
            Self::One(a) => {
                assert_eq!(index, 0);
                *self = Self::Empty;
                a
            }
            Self::Two([a, b]) => match index {
                0 => {
                    *self = Self::One(b);
                    a
                }
                1 => {
                    *self = Self::One(a);
                    b
                }
                _ => panic!("index out of bounds"),
            },
            Self::Heap(_) => {
                let mut vec = self.as_slice().to_vec();
                let removed = vec.remove(index);
                self.set_from_slice(&vec);
                removed
            }
        }
    }

    pub fn replace(&mut self, index: usize, new_child: NodeId) -> NodeId {
        match self {
            Self::Empty => panic!("replace on empty SmallChildren"),
            Self::One(a) => {
                assert_eq!(index, 0);
                core::mem::replace(a, new_child)
            }
            Self::Two(pair) => core::mem::replace(&mut pair[index], new_child),
            Self::Heap(slice) => core::mem::replace(&mut slice[index], new_child),
        }
    }

    pub fn retain_not(&mut self, target: NodeId) {
        match *self {
            Self::Empty => {}
            Self::One(a) => {
                if a == target {
                    *self = Self::Empty;
                }
            }
            Self::Two([a, b]) => {
                if a == target && b == target {
                    *self = Self::Empty;
                } else if a == target {
                    *self = Self::One(b);
                } else if b == target {
                    *self = Self::One(a);
                }
            }
            Self::Heap(_) => {
                if self.as_slice().contains(&target) {
                    let vec: Vec<NodeId> = self.as_slice().iter().copied().filter(|&n| n != target).collect();
                    self.set_from_slice(&vec);
                }
            }
        }
    }

    pub fn heap_bytes(&self) -> usize {
        match self {
            Self::Heap(slice) => core::mem::size_of_val(&**slice),
            _ => 0,
        }
    }
}

/// Iterator over a node's child `NodeId`s.
#[derive(Debug, Clone)]
pub struct SmallChildrenIter<'a>(core::slice::Iter<'a, NodeId>);

impl Iterator for SmallChildrenIter<'_> {
    type Item = NodeId;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().copied()
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for SmallChildrenIter<'_> {
    #[inline(always)]
    fn len(&self) -> usize {
        self.0.len()
    }
}
