//! Read-only vanilla NBT paths and SNBT feedback, shared by data/execute queries.
use mc_nbt::Tag;
use std::borrow::Cow;
use std::fmt::Write;

#[derive(Debug)]
enum Node {
    Key(String),
    Index(i32),
    All,
    Match(Tag),
    Filter(Tag),
}

/// Consume one argument, retaining quoted whitespace and compound/list selectors.
pub(crate) fn argument<'a>(input: &mut &'a str) -> Result<&'a str, String> {
    *input = input.trim_start();
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0_u32;
    let mut end = input.len();
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some() && ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '[' | '{' => depth += 1,
                ']' | '}' => depth = depth.checked_sub(1).ok_or("Invalid NBT path")?,
                _ if ch.is_whitespace() && depth == 0 => {
                    end = index;
                    break;
                }
                _ => {}
            }
        }
    }
    if end == 0 || quote.is_some() || depth != 0 {
        return Err("Invalid or missing argument".into());
    }
    let result = &input[..end];
    *input = &input[end..];
    Ok(result)
}

fn nodes(mut path: &str) -> Result<Vec<Node>, String> {
    let mut result = Vec::new();
    let mut needs_key = true;
    while !path.is_empty() {
        if result.len() >= 512 {
            return Err("NBT path is too deep".into());
        }
        if path.starts_with('{') {
            let (value, length) = crate::command_snbt::parse_prefix(path).map_err(str::to_owned)?;
            result.push(Node::Match(value));
            path = &path[length..];
        } else if path.starts_with('[') {
            if needs_key && !result.is_empty() {
                return Err("Invalid NBT path".into());
            }
            path = &path[1..];
            if let Some(rest) = path.strip_prefix(']') {
                result.push(Node::All);
                path = rest;
            } else if path.starts_with('{') {
                let (value, length) =
                    crate::command_snbt::parse_prefix(path).map_err(str::to_owned)?;
                result.push(Node::Filter(value));
                path = path[length..].strip_prefix(']').ok_or("Invalid NBT path")?;
            } else {
                let end = path.find(']').ok_or("Invalid NBT path")?;
                result.push(Node::Index(
                    path[..end].parse().map_err(|_| "Invalid list index")?,
                ));
                path = &path[end + 1..];
            }
        } else {
            if !needs_key {
                return Err("Invalid NBT path".into());
            }
            let key;
            if path.starts_with(['\'', '"']) {
                let (value, length) =
                    crate::command_snbt::parse_prefix(path).map_err(str::to_owned)?;
                let Tag::String(value) = value else {
                    return Err("Invalid quoted key".into());
                };
                key = value;
                path = &path[length..];
            } else {
                let end = path.find(['.', '[', '{']).unwrap_or(path.len());
                if end == 0 {
                    return Err("Invalid NBT path".into());
                }
                key = path[..end].into();
                path = &path[end..];
            }
            result.push(Node::Key(key));
        }
        needs_key = false;
        if let Some(rest) = path.strip_prefix('.') {
            path = rest;
            needs_key = true;
            if path.is_empty() {
                return Err("Invalid NBT path".into());
            }
        }
    }
    if result.is_empty() {
        return Err("Invalid NBT path".into());
    }
    Ok(result)
}

pub(crate) fn matches(pattern: &Tag, value: &Tag) -> bool {
    match (pattern, value) {
        (Tag::Compound(pattern), Tag::Compound(value)) => pattern.iter().all(|(key, expected)| {
            value
                .iter()
                .any(|(name, actual)| name == key && matches(expected, actual))
        }),
        (Tag::List(pattern), Tag::List(value)) => {
            if pattern.elements.is_empty() {
                value.elements.is_empty()
            } else {
                pattern.elements.iter().all(|expected| {
                    value
                        .elements
                        .iter()
                        .any(|actual| matches(expected, actual))
                })
            }
        }
        _ => pattern == value,
    }
}

fn indexed(tag: &Tag, index: i32) -> Option<Cow<'_, Tag>> {
    let len = match tag {
        Tag::List(list) => list.elements.len(),
        Tag::ByteArray(array) => array.len(),
        Tag::IntArray(array) => array.len(),
        Tag::LongArray(array) => array.len(),
        _ => return None,
    };
    let index = if index < 0 {
        len as i64 + i64::from(index)
    } else {
        i64::from(index)
    };
    let index = usize::try_from(index).ok()?;
    match tag {
        Tag::List(list) => list.elements.get(index).map(Cow::Borrowed),
        Tag::ByteArray(array) => array.get(index).map(|v| Cow::Owned(Tag::Byte(*v))),
        Tag::IntArray(array) => array.get(index).map(|v| Cow::Owned(Tag::Int(*v))),
        Tag::LongArray(array) => array.get(index).map(|v| Cow::Owned(Tag::Long(*v))),
        _ => None,
    }
}

pub(crate) fn get<'a>(root: &'a Tag, path: &str) -> Result<Cow<'a, Tag>, String> {
    let mut values = vec![Cow::Borrowed(root)];
    for node in nodes(path)? {
        let mut next = Vec::new();
        for value in values {
            match (&node, value) {
                (Node::Key(key), Cow::Borrowed(Tag::Compound(fields))) => {
                    next.extend(
                        fields
                            .iter()
                            .find(|(name, _)| name == key)
                            .map(|(_, tag)| Cow::Borrowed(tag)),
                    );
                }
                (Node::Index(index), Cow::Borrowed(tag)) => next.extend(indexed(tag, *index)),
                (Node::All | Node::Filter(_), Cow::Borrowed(Tag::List(list))) => {
                    next.extend(
                        list.elements
                            .iter()
                            .filter(|tag| match &node {
                                Node::Filter(pattern) => matches(pattern, tag),
                                _ => true,
                            })
                            .map(Cow::Borrowed),
                    );
                }
                (Node::All, Cow::Borrowed(tag)) => {
                    let len = match tag {
                        Tag::ByteArray(v) => v.len(),
                        Tag::IntArray(v) => v.len(),
                        Tag::LongArray(v) => v.len(),
                        _ => 0,
                    };
                    next.extend((0..len).filter_map(|index| indexed(tag, index as i32)));
                }
                (Node::Match(pattern), value) if matches(pattern, &value) => next.push(value),
                _ => {}
            }
        }
        if next.is_empty() {
            return Err(format!("Nothing found at path {path}"));
        }
        values = next;
    }
    if values.len() != 1 {
        return Err("This argument accepts a single NBT value".into());
    }
    Ok(values.pop().expect("one NBT value"))
}

/// Mutate selected NBT path values. Callers apply this to a copy before commit.
pub(crate) fn modify(
    root: &mut Tag,
    path: &str,
    operation: &str,
    value: Option<&Tag>,
    index: i32,
) -> Result<i32, String> {
    fn merge(target: &mut Vec<(String, Tag)>, patch: &[(String, Tag)]) {
        for (key, value) in patch {
            if let Some((_, old)) = target.iter_mut().find(|(name, _)| name == key) {
                if let (Tag::Compound(old), Tag::Compound(new)) = (&mut *old, value) {
                    merge(old, new);
                } else {
                    *old = value.clone();
                }
            } else {
                target.push((key.clone(), value.clone()));
            }
        }
    }
    fn apply(
        tag: &mut Tag,
        operation: &str,
        value: Option<&Tag>,
        index: i32,
    ) -> Result<i32, String> {
        let old = tag.clone();
        match operation {
            "set" => *tag = value.ok_or("Missing value")?.clone(),
            "merge" => {
                let (Tag::Compound(target), Some(Tag::Compound(patch))) = (&mut *tag, value) else {
                    return Err("Merge requires compounds".into());
                };
                merge(target, patch);
            }
            "append" | "prepend" | "insert" => {
                let Tag::List(list) = &mut *tag else {
                    return Err("Expected an NBT list".into());
                };
                let value = value.ok_or("Missing value")?;
                if !list.elements.is_empty() && list.element_type != value.type_id() {
                    return Err("NBT list element type mismatch".into());
                }
                list.element_type = value.type_id();
                let at = match operation {
                    "append" => list.elements.len(),
                    "prepend" => 0,
                    _ => {
                        let at = if index < 0 {
                            list.elements.len() as i64 + i64::from(index) + 1
                        } else {
                            i64::from(index)
                        };
                        if at < 0 || at > list.elements.len() as i64 {
                            return Err("Invalid list index".into());
                        }
                        at as usize
                    }
                };
                list.elements.insert(at, value.clone());
            }
            _ => return Err("Unsupported NBT operation".into()),
        }
        Ok(i32::from(*tag != old))
    }
    fn descend(
        tag: &mut Tag,
        nodes: &[Node],
        operation: &str,
        value: Option<&Tag>,
        index: i32,
    ) -> Result<i32, String> {
        if nodes.is_empty() {
            return apply(tag, operation, value, index);
        }
        match (&nodes[0], tag) {
            (Node::Key(key), Tag::Compound(fields)) => {
                let position = fields.iter().position(|(name, _)| name == key);
                let created = position.is_none();
                let position = match position {
                    Some(position) => position,
                    None if operation != "remove" => {
                        let initial = if nodes.len() == 1
                            && matches!(operation, "append" | "prepend" | "insert")
                        {
                            Tag::List(mc_nbt::ListTag {
                                element_type: 0,
                                elements: Vec::new(),
                            })
                        } else {
                            Tag::Compound(Vec::new())
                        };
                        fields.push((key.clone(), initial));
                        fields.len() - 1
                    }
                    None => return Ok(0),
                };
                if nodes.len() == 1 && operation == "remove" {
                    fields.remove(position);
                    return Ok(1);
                }
                let count = descend(
                    &mut fields[position].1,
                    &nodes[1..],
                    operation,
                    value,
                    index,
                )?;
                Ok(if created { count.max(1) } else { count })
            }
            (Node::Index(number), Tag::List(list)) => {
                let at = if *number < 0 {
                    list.elements.len() as i64 + i64::from(*number)
                } else {
                    i64::from(*number)
                };
                if at < 0 || at >= list.elements.len() as i64 {
                    return Ok(0);
                }
                if nodes.len() == 1 && operation == "remove" {
                    list.elements.remove(at as usize);
                    return Ok(1);
                }
                let count = descend(
                    &mut list.elements[at as usize],
                    &nodes[1..],
                    operation,
                    value,
                    index,
                )?;
                if list.elements[at as usize].type_id() != list.element_type {
                    return Err("NBT list element type mismatch".into());
                }
                Ok(count)
            }
            (Node::All | Node::Filter(_), Tag::List(list)) => {
                let selected = |tag: &Tag| match &nodes[0] {
                    Node::Filter(pattern) => matches(pattern, tag),
                    _ => true,
                };
                if nodes.len() == 1 && operation == "remove" {
                    let before = list.elements.len();
                    list.elements.retain(|tag| !selected(tag));
                    return Ok((before - list.elements.len()) as i32);
                }
                let mut count = 0;
                for tag in &mut list.elements {
                    if selected(tag) {
                        count += descend(tag, &nodes[1..], operation, value, index)?;
                        if tag.type_id() != list.element_type {
                            return Err("NBT list element type mismatch".into());
                        }
                    }
                }
                Ok(count)
            }
            (Node::Match(pattern), tag) if matches(pattern, tag) => {
                descend(tag, &nodes[1..], operation, value, index)
            }
            _ => Ok(0),
        }
    }
    let count = descend(root, &nodes(path)?, operation, value, index)?;
    if count == 0 {
        return Err(format!("Nothing changed at path {path}"));
    }
    Ok(count)
}

pub(crate) fn number(tag: &Tag) -> Option<f64> {
    Some(match tag {
        Tag::Byte(v) => f64::from(*v),
        Tag::Short(v) => f64::from(*v),
        Tag::Int(v) => f64::from(*v),
        Tag::Long(v) => *v as f64,
        Tag::Float(v) => f64::from(*v),
        Tag::Double(v) => *v,
        _ => return None,
    })
}

pub(crate) fn result(tag: &Tag) -> i32 {
    if let Some(value) = number(tag) {
        return value.floor() as i32;
    }
    match tag {
        Tag::String(v) => v.encode_utf16().count() as i32,
        Tag::List(v) => v.elements.len() as i32,
        Tag::Compound(v) => v.len() as i32,
        Tag::ByteArray(v) => v.len() as i32,
        Tag::IntArray(v) => v.len() as i32,
        Tag::LongArray(v) => v.len() as i32,
        _ => 0,
    }
}

fn quoted(value: &str, out: &mut String) {
    let quote = value
        .chars()
        .find(|ch| matches!(ch, '\'' | '"'))
        .map_or('"', |ch| if ch == '"' { '\'' } else { '"' });
    out.push(quote);
    for ch in value.chars() {
        if ch == quote || ch == '\\' {
            out.push('\\');
        }
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push(quote);
}

pub(crate) fn snbt(tag: &Tag) -> String {
    fn render(tag: &Tag, out: &mut String) {
        match tag {
            Tag::Byte(v) => {
                write!(out, "{v}b").unwrap();
            }
            Tag::Short(v) => {
                write!(out, "{v}s").unwrap();
            }
            Tag::Int(v) => {
                write!(out, "{v}").unwrap();
            }
            Tag::Long(v) => {
                write!(out, "{v}L").unwrap();
            }
            Tag::Float(v) => {
                write!(out, "{v:?}f").unwrap();
            }
            Tag::Double(v) => {
                write!(out, "{v:?}d").unwrap();
            }
            Tag::String(v) => quoted(v, out),
            Tag::List(v) => {
                out.push('[');
                for (i, value) in v.elements.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    render(value, out);
                }
                out.push(']');
            }
            Tag::Compound(fields) => {
                out.push('{');
                let mut fields: Vec<_> = fields.iter().collect();
                fields.sort_by(|a, b| a.0.cmp(&b.0));
                for (i, (key, value)) in fields.into_iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    if !key.is_empty()
                        && key
                            .bytes()
                            .all(|ch| ch.is_ascii_alphanumeric() || b"._+-".contains(&ch))
                    {
                        out.push_str(key);
                    } else {
                        quoted(key, out);
                    }
                    out.push_str(": ");
                    render(value, out);
                }
                out.push('}');
            }
            Tag::ByteArray(v) => {
                out.push_str("[B;");
                for (i, value) in v.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    write!(out, " {value}B").unwrap();
                }
                out.push(']');
            }
            Tag::IntArray(v) => {
                out.push_str("[I;");
                for (i, value) in v.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    write!(out, " {value}").unwrap();
                }
                out.push(']');
            }
            Tag::LongArray(v) => {
                out.push_str("[L;");
                for (i, value) in v.iter().enumerate() {
                    if i != 0 {
                        out.push(',');
                    }
                    write!(out, " {value}L").unwrap();
                }
                out.push(']');
            }
        }
    }
    let mut out = String::new();
    render(tag, &mut out);
    out
}
