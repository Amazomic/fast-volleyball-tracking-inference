use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};
const MAX_INPUT: usize = 8 * 1024 * 1024;
thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static HASHES: RefCell<BTreeMap<u32,Sha256>> = const { RefCell::new(BTreeMap::new()) };
}
#[no_mangle]
pub extern "C" fn volley_data_reserve(len: u32) -> *mut u8 {
    if len as usize > MAX_INPUT {
        return std::ptr::null_mut();
    }
    INPUT.with(|input| {
        let mut input = input.borrow_mut();
        input.resize(len as usize, 0);
        input.as_mut_ptr()
    })
}
#[no_mangle]
pub extern "C" fn volley_data_output_ptr() -> *const u8 {
    OUTPUT.with(|o| o.borrow().as_ptr())
}
#[no_mangle]
pub extern "C" fn volley_data_output_len() -> u32 {
    OUTPUT.with(|o| o.borrow().len() as u32)
}
fn id(value: &Value) -> bool {
    value.as_str().is_some_and(volley_core::contracts::job_id)
}

fn rows<'a>(overview: &'a Value, stage: &str) -> Vec<&'a Value> {
    overview["stages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["key"] == stage)
        .and_then(|r| r["items"].as_array())
        .into_iter()
        .flatten()
        .filter(|r| id(&r["id"]))
        .collect()
}
fn add(
    choices: &mut Vec<Value>,
    by: &mut BTreeMap<String, Vec<Value>>,
    video: &str,
    kind: &str,
    row: &Value,
) -> String {
    let value = json!([video, kind, row["id"]]).to_string();
    if !by.contains_key(&value) {
        let digest = Sha256::digest(value.as_bytes());
        let first = [
            "Быстрый",
            "Тихий",
            "Яркий",
            "Смелый",
            "Лёгкий",
            "Лунный",
            "Вольный",
            "Верный",
            "Зоркий",
            "Горный",
            "Синий",
            "Ранний",
            "Тёплый",
            "Свежий",
            "Дальний",
            "Светлый",
        ];
        let second = [
            "сокол",
            "ветер",
            "мяч",
            "тигр",
            "луч",
            "кедр",
            "орёл",
            "парус",
            "берег",
            "ястреб",
            "ручей",
            "маяк",
            "океан",
            "гром",
            "атлас",
            "космос",
        ];
        let display_name = format!(
            "{} {}",
            first[digest[0] as usize % first.len()],
            second[digest[1] as usize % second.len()]
        );
        choices.push(json!({"value":value,"video":video,"kind":kind,"id":row["id"],"model":row["model"],"created_at":row["created_at"],"display_name":display_name}));
        by.insert(value.clone(), Vec::new());
    }
    value
}
fn chain(input: &Value) -> Value {
    let mut choices = Vec::new();
    let mut by = BTreeMap::<String, Vec<Value>>::new();
    let mut run_to = BTreeMap::new();
    for video in input["videos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let overview = &input["overviews"][video];
        let fusion = rows(overview, "fusion");
        let gpu = rows(overview, "gpu");
        let replay: BTreeMap<_, _> = rows(overview, "tracking")
            .into_iter()
            .filter(|r| id(&r["source_fusion_id"]))
            .map(|r| {
                (
                    r["id"].as_str().unwrap_or_default(),
                    r["source_fusion_id"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        let mut fusion_keys = BTreeMap::new();
        let mut fused_gpu = BTreeSet::new();
        for row in fusion {
            fusion_keys.insert(
                row["id"].as_str().unwrap_or_default(),
                add(&mut choices, &mut by, video, "fusion", row),
            );
            for key in ["ravel_job_id", "vball_job_id"] {
                if id(&row[key]) {
                    fused_gpu.insert(row[key].as_str().unwrap_or_default());
                }
            }
        }
        for row in gpu {
            if !fused_gpu.contains(row["id"].as_str().unwrap_or_default()) {
                add(&mut choices, &mut by, video, "gpu", row);
            }
        }
        let mut seen = BTreeSet::new();
        for run in input["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["video"] == video && id(&r["rally_run_id"]))
        {
            let run_id = run["rally_run_id"].as_str().unwrap_or_default();
            if !seen.insert(run_id) {
                continue;
            }
            let source = run["source_replay_id"].as_str().unwrap_or_default();
            let key = replay
                .get(source)
                .and_then(|s| fusion_keys.get(s))
                .cloned()
                .unwrap_or_else(|| {
                    add(
                        &mut choices,
                        &mut by,
                        video,
                        "unlinked",
                        &json!({"id":if source.is_empty(){run_id}else{source}}),
                    )
                });
            if let Some(items) = by.get_mut(&key) {
                items.push(run.clone());
            }
            run_to.insert(run_id, key);
        }
    }
    json!({"choices":choices,"byAnalysis":by,"runToAnalysis":run_to})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_choice_has_stable_two_word_display_name_and_keeps_full_id() {
        let input = json!({"videos":["match.mp4"],"overviews":{"match.mp4":{"stages":[
            {"key":"fusion","items":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","created_at":1700000000}]}
        ]}},"runs":[]});
        let result = chain(&input);
        let choice = &result["choices"][0];
        let name = choice["display_name"].as_str().unwrap();
        assert_eq!(name.split_whitespace().count(), 2);
        assert_eq!(choice["id"], "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(
            choice["value"],
            "[\"match.mp4\",\"fusion\",\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"]"
        );
        assert_eq!(
            choice["display_name"],
            chain(&input)["choices"][0]["display_name"]
        );
    }
}
fn montage(input: &Value) -> Value {
    let mut rows: Vec<_> = input.as_array().into_iter().flatten().collect();
    rows.sort_by(|a, b| {
        let start = |v: &Value| {
            v["start"]
                .as_f64()
                .filter(|v| v.is_finite())
                .unwrap_or(f64::INFINITY)
        };
        start(a).total_cmp(&start(b))
    });
    json!(rows.iter().map(|r| &r["id"]).collect::<Vec<_>>())
}
#[no_mangle]
pub extern "C" fn volley_domain_call(operation: u32) -> i32 {
    OUTPUT.with(|out| out.borrow_mut().clear());
    let input = INPUT.with(|i| serde_json::from_slice::<Value>(&i.borrow()));
    let Ok(input) = input else {
        return 0;
    };
    let value = match operation {
        1 => chain(&input),
        2 => montage(&input),
        _ => return 0,
    };
    match serde_json::to_vec(&value) {
        Ok(bytes) => {
            OUTPUT.with(|o| *o.borrow_mut() = bytes);
            1
        }
        Err(_) => 0,
    }
}
#[no_mangle]
pub extern "C" fn volley_hash_begin() -> u32 {
    HASHES.with(|h| {
        let mut h = h.borrow_mut();
        if h.len() >= 32 {
            return 0;
        }
        let Some(id) = (1..=33).find(|id| !h.contains_key(id)) else {
            return 0;
        };
        h.insert(id, Sha256::new());
        id
    })
}
#[no_mangle]
pub extern "C" fn volley_hash_update(id: u32) -> i32 {
    HASHES.with(|h| {
        let mut h = h.borrow_mut();
        let Some(hash) = h.get_mut(&id) else {
            return 0;
        };
        INPUT.with(|i| hash.update(&*i.borrow()));
        1
    })
}
#[no_mangle]
pub extern "C" fn volley_hash_finish(id: u32) -> i32 {
    OUTPUT.with(|o| o.borrow_mut().clear());
    HASHES.with(|h| {
        let Some(hash) = h.borrow_mut().remove(&id) else {
            return 0;
        };
        OUTPUT.with(|o| *o.borrow_mut() = format!("{:x}", hash.finalize()).into_bytes());
        1
    })
}
#[no_mangle]
pub extern "C" fn volley_hash_abort(id: u32) {
    HASHES.with(|h| h.borrow_mut().remove(&id));
}
#[no_mangle]
pub extern "C" fn volley_evidence_fps(metadata: f64) -> f64 {
    if metadata.is_finite() && metadata > 0.0 && metadata <= 1000.0 {
        metadata
    } else {
        30.0
    }
}
