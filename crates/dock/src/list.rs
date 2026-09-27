//! Pure edits of the pinned app id list: insert, move and remove.

/// Put `id` at `index` (clamped to the list length). An id already in the
/// list is moved rather than duplicated; `index` always refers to the list
/// as it was before the edit.
pub fn insert_at(ids: &mut Vec<String>, id: &str, index: usize) {
    let mut index = index.min(ids.len());
    if let Some(old) = ids.iter().position(|x| x == id) {
        ids.remove(old);
        if old < index {
            index -= 1;
        }
    }
    ids.insert(index, id.to_string());
}

/// Remove `id` from the list, if present.
pub fn remove(ids: &mut Vec<String>, id: &str) {
    ids.retain(|x| x != id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn insert_adds_at_the_index() {
        let mut ids = list(&["a", "b"]);
        insert_at(&mut ids, "c", 1);
        assert_eq!(ids, list(&["a", "c", "b"]));
    }

    #[test]
    fn insert_clamps_a_far_index_to_the_end() {
        let mut ids = list(&["a"]);
        insert_at(&mut ids, "b", 99);
        assert_eq!(ids, list(&["a", "b"]));
    }

    #[test]
    fn moving_forward_accounts_for_the_removed_slot() {
        let mut ids = list(&["a", "b", "c"]);
        insert_at(&mut ids, "a", 3);
        assert_eq!(ids, list(&["b", "c", "a"]));
        let mut ids = list(&["a", "b", "c"]);
        insert_at(&mut ids, "a", 2);
        assert_eq!(ids, list(&["b", "a", "c"]));
    }

    #[test]
    fn moving_backward_keeps_the_index() {
        let mut ids = list(&["a", "b", "c"]);
        insert_at(&mut ids, "c", 0);
        assert_eq!(ids, list(&["c", "a", "b"]));
    }

    #[test]
    fn dropping_an_item_on_its_own_slot_changes_nothing() {
        let mut ids = list(&["a", "b", "c"]);
        insert_at(&mut ids, "b", 1);
        assert_eq!(ids, list(&["a", "b", "c"]));
        insert_at(&mut ids, "b", 2);
        assert_eq!(ids, list(&["a", "b", "c"]));
    }

    #[test]
    fn remove_drops_the_id() {
        let mut ids = list(&["a", "b"]);
        remove(&mut ids, "a");
        remove(&mut ids, "zzz");
        assert_eq!(ids, list(&["b"]));
    }
}
