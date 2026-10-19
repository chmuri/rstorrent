use crate::error::RpcError;

pub fn parse_option_flag<F>(option: &str, ftor: F) -> Result<i32, RpcError>
where
    F: Fn(&str) -> Result<i32, RpcError>,
{
    let trimmed = option.trim();
    if trimmed.is_empty() {
        return Err(RpcError::InputError(option.to_string()));
    }

    // Flag must be alphanumeric + '_'
    let is_valid = trimmed.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !is_valid {
        return Err(RpcError::InputError(option.to_string()));
    }

    ftor(trimmed)
}

pub fn parse_option_flags<F>(option: &str, ftor: F, mut flags: i32) -> Result<i32, RpcError>
where
    F: Fn(&str) -> Result<i32, RpcError>,
{
    let trimmed = option.trim();
    if trimmed.is_empty() {
        return Ok(flags);
    }

    let tokens: Vec<&str> = option.split('|').collect();
    for token in tokens {
        let t = token.trim();
        if t.is_empty() || !t.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(RpcError::InputError(option.to_string()));
        }

        let f = ftor(t)?;
        if f < 0 {
            flags &= f;
        } else {
            flags |= f;
        }
    }

    Ok(flags)
}

pub fn parse_option_for_each<F>(option: &str, mut ftor: F) -> Result<(), RpcError>
where
    F: FnMut(&str) -> Result<i32, RpcError>,
{
    let trimmed = option.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let tokens: Vec<&str> = option.split('|').collect();
    for token in tokens {
        let t = token.trim();
        if t.is_empty() || !t.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(RpcError::InputError(option.to_string()));
        }

        ftor(t)?;
    }

    Ok(())
}

pub fn parse_option_print_vector(flags: i32, flag_list: &[(&str, i32)]) -> String {
    let mut result = String::new();

    for (name, f) in flag_list {
        if *f < 0 {
            if (flags & *f) != flags {
                continue;
            }
        } else {
            if (flags & *f) != *f {
                continue;
            }
        }

        if !result.is_empty() {
            result.push('|');
        }
        result.push_str(name);
    }

    result
}

pub fn parse_option_print_flags<F>(mut flags: u32, ftor: F) -> Result<String, RpcError>
where
    F: Fn(u32) -> Result<String, RpcError>,
{
    let mut result = String::new();
    let mut i: u64 = 1;

    while flags != 0 && i <= (1 << 31) {
        let bit = i as u32;
        if flags & bit != 0 {
            if !result.is_empty() {
                result.push('|');
            }
            result.push_str(&ftor(bit)?);
            flags &= !bit;
        }
        i <<= 1;
    }

    if flags != 0 {
        return Err(RpcError::InputError("unknown flag".to_string()));
    }

    Ok(result)
}
