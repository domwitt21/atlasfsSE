use atlasfs_store::Inode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Actor {
    pub user: u32,
    pub primary_group: u32,
    pub supplementary_groups: Vec<u32>,
}

impl Actor {
    #[must_use]
    pub const fn root() -> Self {
        Self {
            user: 0,
            primary_group: 0,
            supplementary_groups: Vec::new(),
        }
    }
    #[must_use]
    pub fn new(user: u32, primary_group: u32, supplementary_groups: Vec<u32>) -> Self {
        Self {
            user,
            primary_group,
            supplementary_groups,
        }
    }
    pub(crate) fn permits(&self, inode: &Inode, requested: u32) -> bool {
        if self.user == 0 {
            return true;
        }
        let shift = if self.user == inode.owner {
            6
        } else if self.primary_group == inode.group
            || self.supplementary_groups.contains(&inode.group)
        {
            3
        } else {
            0
        };
        ((inode.mode >> shift) & requested) == requested
    }
}

pub(crate) const READ: u32 = 4;
pub(crate) const WRITE: u32 = 2;
pub(crate) const EXECUTE: u32 = 1;
