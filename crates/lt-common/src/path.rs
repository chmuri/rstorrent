use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Path {
    components: Vec<String>,
}

impl Path {
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn components(&self) -> &[String] {
        &self.components
    }

    pub fn components_mut(&mut self) -> &mut Vec<String> {
        &mut self.components
    }

    pub fn is_valid_component(name: &str) -> bool {
        if name.is_empty() || name == "." || name == ".." {
            return false;
        }
        !name.chars().any(|c| c == '/' || c == '\0')
    }

    pub fn is_prefix(prefix: &Path, path: &Path) -> bool {
        if prefix.len() > path.len() {
            return false;
        }
        prefix
            .components
            .iter()
            .zip(path.components.iter())
            .all(|(l, r)| l == r)
    }

    pub fn push(&mut self, component: &str) {
        self.components.push(component.to_string());
    }

    pub fn push_back_component(&mut self, name: impl Into<String>) -> Result<(), &'static str> {
        let s = name.into();
        if !Self::is_valid_component(&s) {
            return Err("Invalid path component name.");
        }
        self.components.push(s);
        Ok(())
    }

    pub fn push_back(&mut self, path: &str) -> Result<(), &'static str> {
        if path.contains('\0') {
            return Err("Invalid path, contains null character.");
        }

        for part in path.split('/') {
            self.components.push(part.to_string());
        }
        Ok(())
    }

    pub fn as_string(&self) -> String {
        if self.components.is_empty() {
            return String::new();
        }

        let mut res = String::new();
        for c in &self.components {
            res.push('/');
            res.push_str(c);
        }
        res
    }
}

impl PartialOrd for Path {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Path {
    fn cmp(&self, other: &Self) -> Ordering {
        self.components.cmp(&other.components)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_component() {
        assert!(Path::is_valid_component("foo"));
        assert!(Path::is_valid_component("foo.bar"));
        assert!(!Path::is_valid_component(""));
        assert!(!Path::is_valid_component("."));
        assert!(!Path::is_valid_component(".."));
        assert!(!Path::is_valid_component("foo/bar"));
        assert!(!Path::is_valid_component("foo\0bar"));
    }

    #[test]
    fn test_push_and_as_string() {
        let mut p = Path::new();
        assert_eq!(p.as_string(), "");
        p.push_back("a/b/c").unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p.as_string(), "/a/b/c");

        let mut p2 = Path::new();
        p2.push_back_component("dir").unwrap();
        p2.push_back_component("file.txt").unwrap();
        assert_eq!(p2.as_string(), "/dir/file.txt");
    }

    #[test]
    fn test_prefix_and_cmp() {
        let mut p1 = Path::new();
        p1.push_back("a/b").unwrap();

        let mut p2 = Path::new();
        p2.push_back("a/b/c").unwrap();

        assert!(Path::is_prefix(&p1, &p2));
        assert!(!Path::is_prefix(&p2, &p1));
        assert!(p1 < p2);
    }
}
