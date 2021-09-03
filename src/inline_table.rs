use std::iter::FromIterator;

use crate::key::Key;
use crate::repr::{Decor, InternalString};
use crate::table::{sort_key_value_pairs, Iter, KeyValuePairs, TableKeyValue, TableLike};
use crate::value::{DEFAULT_TRAILING_VALUE_DECOR, DEFAULT_VALUE_DECOR};
use crate::{Item, Value};

/// Type representing a TOML inline table,
/// payload of the `Value::InlineTable` variant
#[derive(Debug, Default, Clone)]
pub struct InlineTable {
    // `preamble` represents whitespaces in an empty table
    pub(crate) preamble: InternalString,
    // prefix before `{` and suffix after `}`
    pub(crate) decor: Decor,
    // whether this is a proxy for dotted keys
    pub(crate) dotted: bool,
    pub(crate) items: KeyValuePairs,
}

impl InlineTable {
    /// Creates an empty table.
    pub fn new() -> Self {
        Default::default()
    }
}

impl InlineTable {
    /// Returns an iterator over key/value pairs.
    pub fn iter(&self) -> InlineTableIter<'_> {
        Box::new(
            self.items
                .iter()
                .filter(|&(_, kv)| kv.value.is_value())
                .map(|(k, kv)| (&k[..], kv.value.as_value().unwrap())),
        )
    }

    /// Returns an iterator over key/value pairs.
    pub fn iter_mut(&mut self) -> InlineTableIterMut<'_> {
        Box::new(
            self.items
                .iter_mut()
                .filter(|(_, kv)| kv.value.is_value())
                .map(|(k, kv)| (&k[..], kv.value.as_value_mut().unwrap())),
        )
    }

    /// Returns the number of key/value pairs.
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// Returns true iff the table is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears the table, removing all key-value pairs. Keeps the allocated memory for reuse.
    pub fn clear(&mut self) {
        self.items.clear()
    }

    /// Gets the given key's corresponding entry in the Table for in-place manipulation.
    pub fn entry<'a>(&'a mut self, key: &str) -> InlineEntry<'a> {
        // Accept a `&str` rather than an owned type to keep `InternalString`, well, internal
        match self.items.entry(key.to_owned()) {
            linked_hash_map::Entry::Occupied(mut entry) => {
                // Ensure it is a `Value` to simplify `InlineOccupiedEntry`'s code.
                let mut scratch = Item::None;
                std::mem::swap(&mut scratch, &mut entry.get_mut().value);
                let mut scratch = Item::Value(
                    scratch
                        .into_value()
                        // HACK: `Item::None` is a corner case of a corner case, let's just pick a
                        // "safe" value
                        .unwrap_or_else(|_| Value::InlineTable(Default::default())),
                );
                std::mem::swap(&mut scratch, &mut entry.get_mut().value);

                InlineEntry::Occupied(InlineOccupiedEntry { entry })
            }
            linked_hash_map::Entry::Vacant(entry) => {
                InlineEntry::Vacant(InlineVacantEntry { entry, key: None })
            }
        }
    }

    /// Gets the given key's corresponding entry in the Table for in-place manipulation.
    pub fn entry_format<'a>(&'a mut self, key: &'a Key) -> InlineEntry<'a> {
        // Accept a `&Key` to be consistent with `entry`
        match self.items.entry(key.get().to_owned()) {
            linked_hash_map::Entry::Occupied(mut entry) => {
                // Ensure it is a `Value` to simplify `InlineOccupiedEntry`'s code.
                let mut scratch = Item::None;
                std::mem::swap(&mut scratch, &mut entry.get_mut().value);
                let mut scratch = Item::Value(
                    scratch
                        .into_value()
                        // HACK: `Item::None` is a corner case of a corner case, let's just pick a
                        // "safe" value
                        .unwrap_or_else(|_| Value::InlineTable(Default::default())),
                );
                std::mem::swap(&mut scratch, &mut entry.get_mut().value);

                InlineEntry::Occupied(InlineOccupiedEntry { entry })
            }
            linked_hash_map::Entry::Vacant(entry) => InlineEntry::Vacant(InlineVacantEntry {
                entry,
                key: Some(key),
            }),
        }
    }
    /// Return an optional reference to the value at the given the key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.items.get(key).and_then(|kv| kv.value.as_value())
    }

    /// Return an optional mutable reference to the value at the given the key.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.items
            .get_mut(key)
            .and_then(|kv| kv.value.as_value_mut())
    }

    /// Returns true iff the table contains given key.
    pub fn contains_key(&self, key: &str) -> bool {
        if let Some(kv) = self.items.get(key) {
            !kv.value.is_none()
        } else {
            false
        }
    }

    /// Inserts a key/value pair if the table does not contain the key.
    /// Returns a mutable reference to the corresponding value.
    pub fn get_or_insert<V: Into<Value>>(&mut self, key: &str, value: V) -> &mut Value {
        let key = Key::with_key(key);
        self.items
            .entry(key.get().to_owned())
            .or_insert(TableKeyValue::new(key, Item::Value(value.into())))
            .value
            .as_value_mut()
            .expect("non-value type in inline table")
    }

    /// Inserts a key-value pair into the map.
    pub fn insert_formatted(&mut self, key: &Key, value: Value) -> Option<Value> {
        let kv = TableKeyValue::new(key.to_owned(), Item::Value(value));
        self.items
            .insert(key.get().to_owned(), kv)
            .filter(|kv| kv.value.is_value())
            .map(|kv| kv.value.into_value().unwrap())
    }

    /// Removes an item given the key.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.items
            .remove(key)
            .and_then(|kv| kv.value.into_value().ok())
    }

    /// Removes a key from the map, returning the stored key and value if the key was previously in the map.
    pub fn remove_entry(&mut self, key: &str) -> Option<(Key, Value)> {
        self.items.remove(key).and_then(|kv| {
            let key = kv.key;
            kv.value.into_value().ok().map(|value| (key, value))
        })
    }
}

impl InlineTable {
    /// Get key/values for values that are visually children of this table
    ///
    /// For example, this will return dotted keys
    pub fn get_values<'s, 'c>(&'s self, children: &'c mut Vec<(Vec<&'s Key>, &'s Value)>) {
        let root = Vec::new();
        self.get_values_internal(&root, children);
    }

    fn get_values_internal<'s, 'c>(
        &'s self,
        parent: &[&'s Key],
        children: &'c mut Vec<(Vec<&'s Key>, &'s Value)>,
    ) {
        for value in self.items.values() {
            let mut path = parent.to_vec();
            path.push(&value.key);
            match &value.value {
                Item::Value(Value::InlineTable(table)) if table.is_dotted() => {
                    table.get_values_internal(&path, children);
                }
                Item::Value(value) => {
                    children.push((path, value));
                }
                _ => {}
            }
        }
    }

    /// Check if this is a wrapper for dotted keys, rather than a standard table
    pub fn is_dotted(&self) -> bool {
        self.dotted
    }

    /// Change this table's dotted status
    pub fn set_dotted(&mut self, yes: bool) {
        self.dotted = yes;
    }

    /// Sorts the key/value pairs by key.
    pub fn sort(&mut self) {
        sort_key_value_pairs(&mut self.items);
    }

    /// Auto formats the table.
    pub fn fmt(&mut self) {
        decorate_inline_table(self);
    }

    /// Returns the decor associated with a given key of the table.
    pub fn decor(&self, key: &str) -> Option<&Decor> {
        self.items.get(key).map(|kv| &kv.key.decor)
    }
}

impl<K: Into<Key>, V: Into<Value>> Extend<(K, V)> for InlineTable {
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (key, value) in iter {
            let key = key.into();
            let value = Item::Value(value.into());
            let value = TableKeyValue::new(key, value);
            self.items.insert(value.key.get().to_owned(), value);
        }
    }
}

impl<K: Into<Key>, V: Into<Value>> FromIterator<(K, V)> for InlineTable {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
    {
        let mut table = InlineTable::new();
        table.extend(iter);
        table
    }
}

impl IntoIterator for InlineTable {
    type Item = (String, Value);
    type IntoIter = InlineTableIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(
            self.items
                .into_iter()
                .filter(|(_, kv)| kv.value.is_value())
                .map(|(k, kv)| (k, kv.value.into_value().unwrap())),
        )
    }
}

impl<'s> IntoIterator for &'s InlineTable {
    type Item = (&'s str, &'s Value);
    type IntoIter = InlineTableIter<'s>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

fn decorate_inline_table(table: &mut InlineTable) {
    let n = table.len();
    for (i, (key_decor, value)) in table
        .items
        .iter_mut()
        .filter(|&(_, ref kv)| kv.value.is_value())
        .map(|(_, kv)| (&mut kv.key.decor, kv.value.as_value_mut().unwrap()))
        .enumerate()
    {
        // { key1 = value1, key2 = value2 }
        *key_decor = Decor::new(DEFAULT_INLINE_KEY_DECOR.0, DEFAULT_INLINE_KEY_DECOR.1);
        if i == n - 1 {
            value.decorate(
                DEFAULT_TRAILING_VALUE_DECOR.0,
                DEFAULT_TRAILING_VALUE_DECOR.1,
            );
        } else {
            value.decorate(DEFAULT_VALUE_DECOR.0, DEFAULT_VALUE_DECOR.1);
        }
    }
}

/// An owned iterator type over key/value pairs of an inline table.
pub type InlineTableIntoIter = Box<dyn Iterator<Item = (String, Value)>>;
/// An iterator type over key/value pairs of an inline table.
pub type InlineTableIter<'a> = Box<dyn Iterator<Item = (&'a str, &'a Value)> + 'a>;
/// A mutable iterator type over key/value pairs of an inline table.
pub type InlineTableIterMut<'a> = Box<dyn Iterator<Item = (&'a str, &'a mut Value)> + 'a>;

impl TableLike for InlineTable {
    fn iter(&self) -> Iter<'_> {
        Box::new(self.items.iter().map(|(key, kv)| (&key[..], &kv.value)))
    }
    fn get<'s>(&'s self, key: &str) -> Option<&'s Item> {
        self.items.get(key).map(|kv| &kv.value)
    }
    fn get_mut<'s>(&'s mut self, key: &str) -> Option<&'s mut Item> {
        self.items.get_mut(key).map(|kv| &mut kv.value)
    }
}

// `{ key1 = value1, ... }`
pub(crate) const DEFAULT_INLINE_KEY_DECOR: (&str, &str) = (" ", " ");

/// A view into a single location in a map, which may be vacant or occupied.
pub enum InlineEntry<'a> {
    /// An occupied Entry.
    Occupied(InlineOccupiedEntry<'a>),
    /// A vacant Entry.
    Vacant(InlineVacantEntry<'a>),
}

impl<'a> InlineEntry<'a> {
    /// Returns the entry key
    ///
    /// # Examples
    ///
    /// ```
    /// use toml_edit::Table;
    ///
    /// let mut map = Table::new();
    ///
    /// assert_eq!("hello", map.entry("hello").key());
    /// ```
    pub fn key(&self) -> &str {
        match self {
            InlineEntry::Occupied(e) => e.key(),
            InlineEntry::Vacant(e) => e.key(),
        }
    }

    /// Ensures a value is in the entry by inserting the default if empty, and returns
    /// a mutable reference to the value in the entry.
    pub fn or_insert(self, default: Value) -> &'a mut Value {
        match self {
            InlineEntry::Occupied(entry) => entry.into_mut(),
            InlineEntry::Vacant(entry) => entry.insert(default),
        }
    }

    /// Ensures a value is in the entry by inserting the result of the default function if empty,
    /// and returns a mutable reference to the value in the entry.
    pub fn or_insert_with<F: FnOnce() -> Value>(self, default: F) -> &'a mut Value {
        match self {
            InlineEntry::Occupied(entry) => entry.into_mut(),
            InlineEntry::Vacant(entry) => entry.insert(default()),
        }
    }
}

/// A view into a single occupied location in a `LinkedHashMap`.
pub struct InlineOccupiedEntry<'a> {
    entry: linked_hash_map::OccupiedEntry<'a, InternalString, TableKeyValue>,
}

impl<'a> InlineOccupiedEntry<'a> {
    /// Gets a reference to the entry key
    ///
    /// # Examples
    ///
    /// ```
    /// use toml_edit::Table;
    ///
    /// let mut map = Table::new();
    ///
    /// assert_eq!("foo", map.entry("foo").key());
    /// ```
    pub fn key(&self) -> &str {
        self.entry.key().as_str()
    }

    /// Gets a reference to the value in the entry.
    pub fn get(&self) -> &Value {
        self.entry.get().value.as_value().unwrap()
    }

    /// Gets a mutable reference to the value in the entry.
    pub fn get_mut(&mut self) -> &mut Value {
        self.entry.get_mut().value.as_value_mut().unwrap()
    }

    /// Converts the OccupiedEntry into a mutable reference to the value in the entry
    /// with a lifetime bound to the map itself
    pub fn into_mut(self) -> &'a mut Value {
        self.entry.into_mut().value.as_value_mut().unwrap()
    }

    /// Sets the value of the entry, and returns the entry's old value
    pub fn insert(&mut self, value: Value) -> Value {
        let mut value = Item::Value(value);
        std::mem::swap(&mut value, &mut self.entry.get_mut().value);
        value.into_value().unwrap()
    }

    /// Takes the value out of the entry, and returns it
    pub fn remove(self) -> Value {
        self.entry.remove().value.into_value().unwrap()
    }
}

/// A view into a single empty location in a `LinkedHashMap`.
pub struct InlineVacantEntry<'a> {
    entry: linked_hash_map::VacantEntry<'a, InternalString, TableKeyValue>,
    key: Option<&'a Key>,
}

impl<'a> InlineVacantEntry<'a> {
    /// Gets a reference to the entry key
    ///
    /// # Examples
    ///
    /// ```
    /// use toml_edit::Table;
    ///
    /// let mut map = Table::new();
    ///
    /// assert_eq!("foo", map.entry("foo").key());
    /// ```
    pub fn key(&self) -> &str {
        self.entry.key().as_str()
    }

    /// Sets the value of the entry with the VacantEntry's key,
    /// and returns a mutable reference to it
    pub fn insert(self, value: Value) -> &'a mut Value {
        let key = self
            .key
            .cloned()
            .unwrap_or_else(|| Key::with_key(self.key()));
        let value = Item::Value(value);
        self.entry
            .insert(TableKeyValue::new(key, value))
            .value
            .as_value_mut()
            .unwrap()
    }
}
