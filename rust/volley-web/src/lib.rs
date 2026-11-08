//! Pure Rust selection state for the first standalone Studio WASM UI.
//! No JS framework or wasm-bindgen dependencies; the browser reads/writes
//! exactly 24 ASCII bytes through a fixed per-thread WASM memory buffer.
//! The backend remains authoritative for source ownership and clip safety.
use std::cell::RefCell;

const ID_BYTES: usize = 24;
const MAX_SELECTED: usize = 80;
thread_local! {
    static BUFFER:RefCell<[u8;ID_BYTES]>=const{RefCell::new([0;ID_BYTES])};
    static SELECTED:RefCell<Vec<[u8;ID_BYTES]>>=const{RefCell::new(Vec::new())};
}
fn valid(id: &[u8; ID_BYTES]) -> bool {
    id.iter()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}
fn push(id: [u8; ID_BYTES]) -> i32 {
    if !valid(&id) {
        return 0;
    }
    SELECTED.with(|items| {
        let mut list = items.borrow_mut();
        if list.contains(&id) {
            return 1;
        }
        if list.len() >= MAX_SELECTED {
            return -1;
        }
        list.push(id);
        1
    })
}
fn erase(id: [u8; ID_BYTES]) -> i32 {
    if !valid(&id) {
        return 0;
    }
    SELECTED.with(|items| {
        items.borrow_mut().retain(|item| item != &id);
    });
    1
}
#[no_mangle]
pub extern "C" fn volley_web_api_version() -> u32 {
    1
}
/// Address of an aligned 24-byte buffer in exported WASM linear memory.
#[no_mangle]
pub extern "C" fn volley_id_buffer_ptr() -> *mut u8 {
    BUFFER.with(|buf| buf.as_ptr().cast::<u8>())
}
#[no_mangle]
pub extern "C" fn volley_selection_clear() {
    SELECTED.with(|list| list.borrow_mut().clear());
}
#[no_mangle]
pub extern "C" fn volley_selection_count() -> u32 {
    SELECTED.with(|list| list.borrow().len() as u32)
}
#[no_mangle]
pub extern "C" fn volley_selection_limit() -> u32 {
    MAX_SELECTED as u32
}
#[no_mangle]
pub extern "C" fn volley_selection_add() -> i32 {
    BUFFER.with(|buf| push(*buf.borrow()))
}
#[no_mangle]
pub extern "C" fn volley_selection_remove() -> i32 {
    BUFFER.with(|buf| erase(*buf.borrow()))
}
#[no_mangle]
pub extern "C" fn volley_selection_contains() -> i32 {
    BUFFER.with(|buf| {
        let id = *buf.borrow();
        if !valid(&id) {
            return 0;
        }
        SELECTED.with(|items| i32::from(items.borrow().contains(&id)))
    })
}
/// Copy ID at index into the shared 24-byte input buffer. Return 1 if found.
#[no_mangle]
pub extern "C" fn volley_selection_at(index: u32) -> i32 {
    let found = SELECTED.with(|list| list.borrow().get(index as usize).copied());
    match found {
        Some(id) => {
            BUFFER.with(|buf| *buf.borrow_mut() = id);
            1
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(n: usize) -> [u8; ID_BYTES] {
        let value = format!("{n:024x}");
        let mut id = [0; ID_BYTES];
        id.copy_from_slice(value.as_bytes());
        id
    }
    #[test]
    fn choices_are_insertion_ordered_deduplicated_and_removable() {
        volley_selection_clear();
        assert_eq!(push(candidate(15)), 1);
        assert_eq!(push(candidate(1)), 1);
        assert_eq!(push(candidate(15)), 1);
        assert_eq!(volley_selection_count(), 2);
        assert_eq!(volley_selection_at(0), 1);
        BUFFER.with(|b| assert_eq!(*b.borrow(), candidate(15)));
        assert_eq!(volley_selection_at(1), 1);
        BUFFER.with(|b| assert_eq!(*b.borrow(), candidate(1)));
        assert_eq!(erase(candidate(15)), 1);
        assert_eq!(volley_selection_count(), 1);
        assert_eq!(volley_selection_at(0), 1);
        BUFFER.with(|b| assert_eq!(*b.borrow(), candidate(1)));
        volley_selection_clear();
        assert_eq!(volley_selection_count(), 0);
    }
    #[test]
    fn rejects_nonhex_and_enforces_eighty_limit() {
        volley_selection_clear();
        for n in 0..80 {
            assert_eq!(push(candidate(n)), 1);
        }
        assert_eq!(push(candidate(80)), -1);
        assert_eq!(volley_selection_count(), 80);
        assert_eq!(push(candidate(0)), 1);
        let mut bad = candidate(4);
        bad[0] = b'/';
        assert_eq!(push(bad), 0);
        assert_eq!(erase(bad), 0);
        assert_eq!(volley_selection_count(), 80);
        volley_selection_clear();
    }
}
