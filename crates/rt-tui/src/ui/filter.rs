use lt_common::Object;

/// Wrap a user-typed download name pattern in the anchors 'match' expects.
pub fn create_filter_pattern(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut pattern = input.to_lowercase();
    if !pattern.ends_with('$') {
        pattern.push_str(".*");
    }
    if !pattern.starts_with('^') {
        pattern = format!(".*{}", pattern);
    }

    pattern
}

/// Build the 'match' command the temporary view filter runs against d.name.
pub fn create_filter_command(pattern: &str) -> Object {
    let name = Object::create_dict_key("d.name".as_bytes().to_vec(), Object::create_empty());
    let args = vec![name, Object::from(pattern)];
    Object::create_dict_key("match".as_bytes().to_vec(), Object::create_list_with(args))
}
