//! Pure Rust selection state for the first standalone Studio WASM UI.
//! No JS framework or wasm-bindgen dependencies; the browser reads/writes
//! exactly 24 ASCII bytes through a fixed per-thread WASM memory buffer.
//! The backend remains authoritative for source ownership and clip safety.
mod domain;
mod preview_scrub;
use std::cell::{Cell, RefCell};

const ID_BYTES: usize = 24;
const MAX_SELECTED: usize = 80;
thread_local! {
    static BUFFER:RefCell<[u8;ID_BYTES]>=const{RefCell::new([0;ID_BYTES])};
    static SELECTED:RefCell<Vec<[u8;ID_BYTES]>>=const{RefCell::new(Vec::new())};
}
fn valid(id: &[u8; ID_BYTES]) -> bool {
    std::str::from_utf8(id).is_ok_and(volley_core::contracts::candidate_id)
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

/// Authoritative WASM state for navigation and browser-only UI preferences.
/// JS remains the platform adapter for DOM, files, media and same-origin HTTP.
const UI_TAB_COUNT: u32 = 6;
const TEXT_BYTES: usize = 512;
const EXTRA_TEXT: &str = include_str!("studio_i18n.tsv");
#[derive(Clone, Copy)]
struct UiState {
    tab: u32,
    locale: u32,
    theme: u32,
    density: u32,
    reduced_motion: u32,
}
impl UiState {
    const fn initial() -> Self {
        Self {
            tab: 0,
            locale: 0,
            theme: 0,
            density: 0,
            reduced_motion: 0,
        }
    }
    fn set(&mut self, key: u32, value: u32) -> bool {
        let field = match key {
            0 => (&mut self.tab, UI_TAB_COUNT - 1),
            1 => (&mut self.locale, 1),
            2 => (&mut self.theme, 1),
            3 => (&mut self.density, 1),
            4 => (&mut self.reduced_motion, 1),
            _ => return false,
        };
        if value > field.1 {
            return false;
        }
        *field.0 = value;
        true
    }
    fn get(&self, key: u32) -> Option<u32> {
        match key {
            0 => Some(self.tab),
            1 => Some(self.locale),
            2 => Some(self.theme),
            3 => Some(self.density),
            4 => Some(self.reduced_motion),
            _ => None,
        }
    }
}
/// Numeric IDs are a stable RU/EN translation API, no dynamic HTML.
const TEXT: &[(&str, &str)] = &[
    ("Предпросмотр", "Preview"),
    ("Анализ", "Analysis"),
    ("Игроки", "Players"),
    ("Отчёт", "Report"),
    ("Монтаж", "Montage"),
    ("Быстрый рилс", "Quick Reel"),
    (
        "Мин. кадров для подтверждения ID",
        "Minimum observed frames to confirm ID",
    ),
    (
        "ID с меньшим числом реальных обнаружений удаляется из CPU-результата",
        "IDs with fewer actual observations are removed from CPU output",
    ),
    ("▶ Начать анализ", "▶ Start analysis"),
    (
        "Восстановление Track ID, кадров (90 = 3 сек)",
        "Track ID recovery, frames (90 = 3 sec)",
    ),
    (
        "Запустить только GPU и создать отдельную версию первичного анализа",
        "Run GPU only and create a separate primary analysis version",
    ),
    (
        "Запустить CPU на выбранной в шапке GPU-версии и сохранить новую постобработку",
        "Run CPU on the GPU version selected in the header and save a new postprocessing version",
    ),
    ("Проект и анализ", "Projects & analysis"),
    ("Проект", "Project"),
    ("CPU-версия розыгрышей", "CPU rally version"),
    ("Обновить данные", "Refresh data"),
    ("Навигация", "Navigation"),
    ("Отдельная Rust-разработка", "Independent Rust development"),
    ("Студия", "Studio"),
    ("ЭКСПЕРИМЕНТАЛЬНАЯ ВЕРСИЯ", "EXPERIMENTAL VERSION"),
    ("Предпросмотр розыгрышей", "Rally preview"),
    ("Розыгрышей", "Rallies"),
    ("Золотых", "Gold-marked"),
    ("Для монтажа", "For montage"),
    ("Все розыгрыши", "All rallies"),
    ("Только золотые", "Gold-marked only"),
    ("Без золотой отметки", "Unmarked only"),
    ("Просмотр видео", "Video preview"),
    ("Выберите карточку розыгрыша", "Choose a rally card"),
    ("Розыгрыш не выбран", "No rally selected"),
    ("Выбранные розыгрыши", "Selected rallies"),
    ("Кодирование", "Encoding"),
    ("Формат ролика", "Video layout"),
    ("Автоматически", "Automatic"),
    (
        "Оригинальный (без масштабирования)",
        "Original (no scaling)",
    ),
    (
        "Вертикальный 9:16 (весь кадр и поля)",
        "Vertical 9:16 (full frame, padded)",
    ),
    ("Аудиотрек (необязательно)", "Soundtrack (optional)"),
    ("Громкость музыкальной дорожки", "Soundtrack volume"),
    ("Создать монтаж", "Render montage"),
    ("Отменить монтаж", "Cancel montage"),
    ("Скачать MP4", "Download MP4"),
    ("Видео проекта", "Project video"),
    ("Обновить стадии", "Refresh stages"),
    ("Запустить VballNet GPU", "Start VballNet GPU"),
    ("Запустить RAVEL GPU", "Start RAVEL GPU"),
    ("Отменить GPU-задачу", "Cancel GPU job"),
    ("Библиотека игроков", "Player library"),
    ("Новый игрок", "New player"),
    ("Добавить", "Add"),
    ("Библиотека", "Library"),
    ("Обновить", "Refresh"),
    ("Исходные видео", "Source videos"),
    ("Выбрать файлы браузером", "Choose files in browser"),
    ("Очистить выбор", "Clear selection"),
    (
        "Музыка для рилса (10–600 секунд)",
        "Reel music (10–600 seconds)",
    ),
    ("Создать быстрый рилс", "Create Quick Reel"),
    ("Обработка", "Processing"),
    ("Отменить обработку", "Cancel processing"),
    ("Проверить состояние", "Check status"),
    ("Скачать готовый MP4", "Download finished MP4"),
    ("Подготовка", "Preparation"),
    ("CPU", "CPU"),
    ("Выберите проект", "Select a project"),
    ("Выберите видео", "Select a video"),
    ("Выберите игрока", "Select a player"),
    ("Данные загружаются…", "Loading data…"),
    ("RU", "RU"),
    ("EN", "EN"),
    ("Результаты обработки", "Processing results"),
    (
        "Выберите проект для загрузки CPU-розыгрышей.",
        "Choose a project to load CPU rallies.",
    ),
    (
        "В проекте ещё нет сохранённых CPU-розыгрышей.",
        "This project has no saved CPU rallies yet.",
    ),
    (
        "Выберите проект и видео для просмотра сохранённых стадий.",
        "Select project and video to review saved stages.",
    ),
    ("Нет сохранённых CPU-версий", "No saved CPU versions"),
    ("Ручное подтверждение", "Manual verification"),
    ("Сохранить", "Save"),
    ("Отмена", "Cancel"),
    ("Открыть", "Open"),
    ("Удалить", "Delete"),
    ("Загрузка проектов…", "Loading projects…"),
    ("Вернуться к предпросмотру", "Return to preview"),
];
thread_local! {
    static UI:RefCell<UiState>=const{RefCell::new(UiState::initial())};
    static UI_TEXT:RefCell<[u8;TEXT_BYTES]>=const{RefCell::new([0;TEXT_BYTES])};
    static UI_LEN:Cell<u32>=const{Cell::new(0)};
    static UI_LOAD:RefCell<[u32;UI_TAB_COUNT as usize]>=const{RefCell::new([0;UI_TAB_COUNT as usize])};
}
#[no_mangle]
pub extern "C" fn volley_ui_api_version() -> u32 {
    1
}
#[no_mangle]
pub extern "C" fn volley_ui_tab_count() -> u32 {
    UI_TAB_COUNT
}
#[no_mangle]
pub extern "C" fn volley_ui_get(key: u32) -> i32 {
    UI.with(|ui| ui.borrow().get(key).map_or(-1, |v| v as i32))
}
#[no_mangle]
pub extern "C" fn volley_ui_set(key: u32, value: u32) -> i32 {
    UI.with(|ui| i32::from(ui.borrow_mut().set(key, value)))
}
#[no_mangle]
pub extern "C" fn volley_ui_reset() {
    UI.with(|ui| *ui.borrow_mut() = UiState::initial());
    UI_LOAD.with(|loads| *loads.borrow_mut() = [0; UI_TAB_COUNT as usize]);
}
#[no_mangle]
pub extern "C" fn volley_ui_begin_load(tab: u32) -> u32 {
    if tab >= UI_TAB_COUNT {
        return 0;
    }
    UI_LOAD.with(|loads| {
        let mut values = loads.borrow_mut();
        let entry = &mut values[tab as usize];
        *entry = entry.wrapping_add(1).max(1);
        *entry
    })
}
#[no_mangle]
pub extern "C" fn volley_ui_current_load(tab: u32, token: u32) -> i32 {
    if tab >= UI_TAB_COUNT || token == 0 {
        return 0;
    }
    UI_LOAD.with(|loads| i32::from(loads.borrow()[tab as usize] == token))
}
#[no_mangle]
pub extern "C" fn volley_ui_text_count() -> u32 {
    (TEXT.len() + EXTRA_TEXT.lines().filter(|l| !l.is_empty()).count()) as u32
}
#[no_mangle]
pub extern "C" fn volley_ui_text_ptr() -> *const u8 {
    UI_TEXT.with(|buf| buf.as_ptr().cast::<u8>())
}
#[no_mangle]
pub extern "C" fn volley_ui_text_len() -> u32 {
    UI_LEN.with(Cell::get)
}
#[no_mangle]
pub extern "C" fn volley_ui_text_copy(key: u32) -> i32 {
    let extra = || {
        EXTRA_TEXT
            .lines()
            .filter(|line| !line.is_empty())
            .nth(key as usize - TEXT.len())
            .and_then(|line| line.split_once('\t'))
    };
    let pair = if (key as usize) < TEXT.len() {
        TEXT.get(key as usize).copied()
    } else {
        extra()
    };
    let Some((ru, en)) = pair else {
        UI_LEN.with(|v| v.set(0));
        return 0;
    };
    let locale = UI.with(|ui| ui.borrow().locale);
    let data = if locale == 0 {
        ru.as_bytes()
    } else {
        en.as_bytes()
    };
    if data.len() > TEXT_BYTES {
        UI_LEN.with(|v| v.set(0));
        return 0;
    }
    UI_TEXT.with(|buf| buf.borrow_mut()[..data.len()].copy_from_slice(data));
    UI_LEN.with(|v| v.set(data.len() as u32));
    1
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
    fn navigation_preferences_and_stale_response_tokens_are_validated_in_rust() {
        volley_ui_reset();
        assert_eq!(volley_ui_tab_count(), 6);
        assert_eq!(volley_ui_set(0, 4), 1);
        assert_eq!(volley_ui_get(0), 4);
        assert_eq!(volley_ui_set(0, 5), 1);
        assert_eq!(volley_ui_get(0), 5);
        assert_eq!(volley_ui_set(0, 6), 0);
        for key in 1..=4 {
            assert_eq!(volley_ui_set(key, 1), 1);
            assert_eq!(volley_ui_get(key), 1);
            assert_eq!(volley_ui_set(key, 2), 0);
        }
        assert_eq!(volley_ui_set(5, 0), 0);
        assert_eq!(volley_ui_get(5), -1);
        let first = volley_ui_begin_load(2);
        assert_ne!(first, 0);
        assert_eq!(volley_ui_current_load(2, first), 1);
        let second = volley_ui_begin_load(2);
        assert_eq!(volley_ui_current_load(2, first), 0);
        assert_eq!(volley_ui_current_load(2, second), 1);
        assert_eq!(volley_ui_current_load(6, second), 0);
        assert_eq!(volley_ui_begin_load(6), 0);
        assert_eq!(volley_ui_text_copy(0), 1);
        UI_TEXT.with(|buf| assert_eq!(&buf.borrow()[..volley_ui_text_len() as usize], b"Preview"));
        assert_eq!(volley_ui_text_copy(volley_ui_text_count()), 0);
        assert_eq!(volley_ui_text_len(), 0);
        volley_ui_reset();
        assert_eq!(volley_ui_get(1), 0);
        assert_eq!(volley_ui_text_copy(0), 1);
        UI_TEXT.with(|buf| {
            assert_eq!(
                &buf.borrow()[..volley_ui_text_len() as usize],
                "Предпросмотр".as_bytes()
            )
        });
        assert_eq!(volley_ui_current_load(2, second), 0);
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
