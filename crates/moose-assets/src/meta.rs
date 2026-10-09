//! Meta values: numbers a level puts on itself, its sectors, surfaces and entities
//! (`$KEY=VALUE` options; an entity's over its template's), for materials to read (a
//! material input's `face:KEY`, `sector:KEY`, `entity:KEY` or `level:KEY` source) and, later,
//! scripts to change. Keys are interned per level, so reading one is an index, not a string.

/// A level's meta keys: each name once, by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaKeys {
    names: Vec<String>,
}

impl MetaKeys {
    /// The id of key `name`, added if it is new.
    pub fn intern(&mut self, name: &str) -> u16 {
        match self.id(name) {
            Some(id) => id,
            None => {
                self.names.push(name.to_string());
                (self.names.len() - 1) as u16
            }
        }
    }

    /// The id of key `name`, if any of the level's things has it.
    pub fn id(&self, name: &str) -> Option<u16> {
        self.names.iter().position(|n| n == name).map(|k| k as u16)
    }

    pub fn name(&self, id: u16) -> &str {
        &self.names[id as usize]
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// One thing's meta values: by key id, each one or more numbers as written (`$wet=200`,
/// `$team=204,51,51`). What they mean is the reader's (a material input's type).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaValues(pub Vec<(u16, Vec<f32>)>);

impl MetaValues {
    pub fn get(&self, key: u16) -> Option<&[f32]> {
        self.0.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_slice())
    }

    /// Sets key `key` to `values` (a script's change, say).
    pub fn set(&mut self, key: u16, values: Vec<f32>) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = values,
            None => self.0.push((key, values)),
        }
    }
}

/// Whether option `option` is a meta value (`$KEY=VALUE`).
pub fn is_meta(option: &str) -> bool {
    option.starts_with('$')
}

/// Reads meta values from `options` (interning their keys in `keys`), returning them and
/// the options that aren't meta values. Later ones win over earlier ones with the same key
/// (an entity's options follow its template's).
pub fn take_meta<'a>(
    keys: &mut MetaKeys,
    options: impl IntoIterator<Item = &'a String>,
) -> Result<(MetaValues, Vec<&'a String>), String> {
    let (mut meta, mut rest) = (MetaValues::default(), Vec::new());
    for option in options {
        let Some(body) = option.strip_prefix('$') else {
            rest.push(option);
            continue;
        };
        let (key, value) = body
            .split_once('=')
            .ok_or_else(|| format!("'{option}' is not a meta value ($KEY=VALUE)"))?;
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("'{key}' is not a meta key (letters, digits and '_')"));
        }
        meta.set(keys.intern(key), parse_numbers(value).map_err(|m| format!("${key}: {m}"))?);
    }
    Ok((meta, rest))
}

/// Numbers separated by commas (`64`, `0.8`, `204,51,51`).
pub fn parse_numbers(text: &str) -> Result<Vec<f32>, String> {
    text.split(',')
        .map(|t| t.parse::<f32>().ok().filter(|x| x.is_finite()).ok_or(format!("'{t}' is not a number")))
        .collect()
}

/// Numbers as [`parse_numbers`] reads them.
pub fn numbers_text(values: &[f32]) -> String {
    values.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_values_are_read_from_options_and_interned() {
        let mut keys = MetaKeys::default();
        let options: Vec<String> =
            ["static", "$wet=200", "$team=204,51,51", "$wet=10"].map(String::from).to_vec();
        let (meta, rest) = take_meta(&mut keys, &options).unwrap();
        assert_eq!(rest, [&options[0]]);
        assert_eq!(keys.len(), 2);
        assert_eq!(meta.get(keys.id("wet").unwrap()), Some(&[10.0][..]));
        assert_eq!(meta.get(keys.id("team").unwrap()), Some(&[204.0, 51.0, 51.0][..]));
        assert!(take_meta(&mut keys, &["$wet".to_string()]).is_err());
        assert!(take_meta(&mut keys, &["$a-b=1".to_string()]).is_err());
        assert!(take_meta(&mut keys, &["$wet=lots".to_string()]).is_err());
    }
}
