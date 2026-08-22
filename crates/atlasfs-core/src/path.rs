use crate::{Error, Result};
use std::fmt;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VirtualPath {
    pub(crate) components: Vec<String>,
}

impl VirtualPath {
    /// Parses a strict, absolute `AtlasFS` path.
    /// # Errors
    /// Returns an error for relative, ambiguous, oversized, or invalid paths.
    pub fn parse(value: &str) -> Result<Self> {
        if !value.starts_with('/')
            || value.len() > 4096
            || value.contains('\0')
            || value.contains('\\')
        {
            return Err(Error::InvalidPath(value.to_owned()));
        }
        if value == "/" {
            return Ok(Self {
                components: Vec::new(),
            });
        }
        if value.ends_with('/') {
            return Err(Error::InvalidPath(value.to_owned()));
        }
        let mut components = Vec::new();
        for component in value[1..].split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.len() > 255
            {
                return Err(Error::InvalidPath(value.to_owned()));
            }
            components.push(component.to_owned());
        }
        Ok(Self { components })
    }
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }
    #[must_use]
    pub fn components(&self) -> &[String] {
        &self.components
    }
    pub(crate) fn split_parent(&self) -> Result<(&[String], &str)> {
        self.components
            .split_last()
            .map(|(name, parent)| (parent, name.as_str()))
            .ok_or(Error::InvalidOperation("root has no parent"))
    }
    #[must_use]
    pub fn starts_with(&self, other: &Self) -> bool {
        self.components.starts_with(&other.components)
    }
}

impl fmt::Display for VirtualPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            formatter.write_str("/")
        } else {
            write!(formatter, "/{}", self.components.join("/"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_canonical_absolute_paths() {
        assert!(VirtualPath::parse("/").unwrap().is_root());
        assert_eq!(VirtualPath::parse("/a/b").unwrap().components(), ["a", "b"]);
    }
    #[test]
    fn rejects_ambiguous_paths() {
        for value in ["a", "//a", "/a/", "/a/../b", "/a\\b"] {
            assert!(VirtualPath::parse(value).is_err(), "accepted {value}");
        }
    }
}
