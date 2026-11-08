//! Strict, provider-independent decoding of RAVEL-013 and VballNetV4c ONNX
//! output ABI into **unlinked, unconfirmed** per-frame detection proposals.
//! No player Track ID or confirmed Touch is assigned by this layer.
use crate::Output;
use serde::Serialize;

const FRAMES: usize = 9;
const MAX_QUERIES: usize = 1024;
const MAX_GRID: usize = 256;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RawDetection {
    pub frame_index: u64,
    pub class_name: &'static str,
    pub score: f32,
    pub bbox_xyxy_norm: [f32; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch_probability: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_point_norm: Option<[f32; 2]>,
}

#[derive(Clone, Copy, Debug)]
pub struct DecodeConfig {
    /// Raw proposals must remain accessible for later CPU threshold tuning.
    pub player_floor: f32,
    pub ball_floor: f32,
    pub max_players: usize,
}
impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            player_floor: 0.05,
            ball_floor: 0.05,
            max_players: 32,
        }
    }
}
impl DecodeConfig {
    fn validate(self) -> Result<(), String> {
        if !self.player_floor.is_finite()
            || !(0.0..=1.0).contains(&self.player_floor)
            || !self.ball_floor.is_finite()
            || !(0.0..=1.0).contains(&self.ball_floor)
            || !(1..=MAX_QUERIES).contains(&self.max_players)
        {
            return Err("Invalid raw detection thresholds or player cap".into());
        }
        Ok(())
    }
}

fn required<'a>(outputs: &'a [Output], name: &str) -> Result<&'a Output, String> {
    outputs
        .iter()
        .find(|o| o.name == name)
        .ok_or_else(|| format!("ONNX output '{name}' is missing"))
}
fn optional<'a>(outputs: &'a [Output], name: &str) -> Option<&'a Output> {
    outputs.iter().find(|o| o.name == name)
}
fn dims(output: &Output, expected: &[i64]) -> Result<(), String> {
    if output.dimensions != expected {
        return Err(format!(
            "Invalid {} shape: {:?}, expected {expected:?}",
            output.name, output.dimensions
        ));
    }
    let len = expected
        .iter()
        .try_fold(1_usize, |a, &x| a.checked_mul(usize::try_from(x).ok()?))
        .ok_or_else(|| format!("Invalid {} element count", output.name))?;
    if output.data.len() != len {
        return Err(format!(
            "Incomplete output '{}': {} != {len}",
            output.name,
            output.data.len()
        ));
    }
    Ok(())
}
fn rect(cx: f32, cy: f32, w: f32, h: f32) -> Option<[f32; 4]> {
    if ![cx, cy, w, h].iter().all(|n| n.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let b = [
        (cx - w / 2.0).clamp(0.0, 1.0),
        (cy - h / 2.0).clamp(0.0, 1.0),
        (cx + w / 2.0).clamp(0.0, 1.0),
        (cy + h / 2.0).clamp(0.0, 1.0),
    ];
    if b[2] <= b[0] || b[3] <= b[1] {
        None
    } else {
        Some(b)
    }
}
fn stable_player_probability(a: f32, b: f32) -> Option<f32> {
    if a.is_finite() && b.is_finite() {
        Some(1.0 / (1.0 + (a - b).clamp(-80.0, 80.0).exp()))
    } else {
        None
    }
}

/// VballNet 18 planes: first nine heatmaps + nine radius maps. Store
/// candidate coordinates and the model radius estimate for every frame.
pub fn decode_vball(
    outputs: &[Output],
    first_frame: u64,
    confidence_floor: f32,
) -> Result<Vec<RawDetection>, String> {
    if !confidence_floor.is_finite() || !(0.0..=1.0).contains(&confidence_floor) {
        return Err("Invalid Vball confidence floor".into());
    }
    let tensor = required(outputs, "heatmaps")?;
    dims(tensor, &[1, 18, 288, 512])?;
    let each = 288 * 512;
    let mut result = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES {
        let map = &tensor.data[frame * each..(frame + 1) * each];
        let Some((index, &score)) = map
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_finite())
            .max_by(|a, b| a.1.total_cmp(b.1))
        else {
            continue;
        };
        if score < confidence_floor || !(0.0..=1.0).contains(&score) {
            continue;
        }
        let radius = tensor.data[(frame + FRAMES) * each + index];
        if !radius.is_finite() || radius <= 0.0 {
            continue;
        }
        let x = (index % 512) as f32 / 512.0;
        let y = (index / 512) as f32 / 288.0;
        if let Some(bbox) = rect(x, y, radius * 2.0, radius * 2.0 * 512.0 / 288.0) {
            result.push(RawDetection {
                frame_index: first_frame + frame as u64,
                class_name: "ball",
                score,
                bbox_xyxy_norm: bbox,
                query_index: None,
                touch_probability: None,
                head_point_norm: None,
            });
        }
    }
    Ok(result)
}

/// RAVEL logits [1,9,Q,2], boxes [1,9,Q,4] in cxcywh, optional
/// action log-probabilities and head points, ball_grid [1,9,3,H,W].
/// Local query_index is NOT a stable identity across windows.
pub fn decode_ravel(
    outputs: &[Output],
    first_frame: u64,
    config: DecodeConfig,
) -> Result<Vec<RawDetection>, String> {
    config.validate()?;
    let logits = required(outputs, "logits")?;
    if logits.dimensions.len() != 4
        || logits.dimensions[0] != 1
        || logits.dimensions[1] != FRAMES as i64
        || logits.dimensions[3] != 2
        || !(1..=MAX_QUERIES as i64).contains(&logits.dimensions[2])
    {
        return Err(format!(
            "Invalid RAVEL player logits shape: {:?}",
            logits.dimensions
        ));
    }
    let q = usize::try_from(logits.dimensions[2]).map_err(|_| "RAVEL queries invalid")?;
    dims(logits, &[1, FRAMES as i64, q as i64, 2])?;
    let boxes = required(outputs, "boxes")?;
    dims(boxes, &[1, FRAMES as i64, q as i64, 4])?;
    let actions = optional(outputs, "action_logits");
    if let Some(output) = actions {
        dims(output, &[1, FRAMES as i64, q as i64, 2])?;
    }
    let heads = optional(outputs, "head_point_norm");
    if let Some(output) = heads {
        dims(output, &[1, FRAMES as i64, q as i64, 2])?;
    }
    let ball_grid = required(outputs, "ball_grid")?;
    if ball_grid.dimensions.len() != 5
        || ball_grid.dimensions[0] != 1
        || ball_grid.dimensions[1] != FRAMES as i64
        || ball_grid.dimensions[2] != 3
        || !(1..=MAX_GRID as i64).contains(&ball_grid.dimensions[3])
        || !(1..=MAX_GRID as i64).contains(&ball_grid.dimensions[4])
    {
        return Err(format!(
            "Invalid RAVEL ball grid shape: {:?}",
            ball_grid.dimensions
        ));
    }
    let gh = usize::try_from(ball_grid.dimensions[3]).map_err(|_| "Grid height invalid")?;
    let gw = usize::try_from(ball_grid.dimensions[4]).map_err(|_| "Grid width invalid")?;
    dims(ball_grid, &[1, FRAMES as i64, 3, gh as i64, gw as i64])?;
    let ball_size = optional(outputs, "ball_size");
    if let Some(output) = ball_size {
        dims(output, &[1, FRAMES as i64, 2, gh as i64, gw as i64])?;
    }
    let mut result = Vec::with_capacity(FRAMES * (config.max_players + 1));
    let plane = gh * gw;
    for frame in 0..FRAMES {
        let mut players = Vec::with_capacity(q.min(config.max_players));
        for query in 0..q {
            let k = frame * q + query;
            let Some(score) = stable_player_probability(logits.data[k * 2], logits.data[k * 2 + 1])
            else {
                continue;
            };
            if score < config.player_floor {
                continue;
            }
            let Some(bbox) = rect(
                boxes.data[k * 4],
                boxes.data[k * 4 + 1],
                boxes.data[k * 4 + 2],
                boxes.data[k * 4 + 3],
            ) else {
                continue;
            };
            let touch_probability = actions.and_then(|tensor| {
                // RAVEL-013 exports LOG probability, not a raw logit.
                let logp = tensor.data[k * 2];
                if logp.is_finite() && logp <= 0.0 {
                    Some(logp.exp().clamp(0.0, 1.0))
                } else {
                    None
                }
            });
            let head_point_norm = heads.and_then(|tensor| {
                let x = tensor.data[k * 2];
                let y = tensor.data[k * 2 + 1];
                if x.is_finite() && y.is_finite() {
                    Some([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)])
                } else {
                    None
                }
            });
            players.push(RawDetection {
                frame_index: first_frame + frame as u64,
                class_name: "player",
                score,
                bbox_xyxy_norm: bbox,
                query_index: Some(query),
                touch_probability,
                head_point_norm,
            });
        }
        // The raw sidecar deliberately keeps overlapping detections; CPU
        // applies NMS and per-run track continuity without rerunning the GPU.
        players.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.query_index.cmp(&b.query_index))
        });
        players.truncate(config.max_players);
        result.extend(players);

        let frame_offset = frame * 3 * plane;
        let confidences = &ball_grid.data[frame_offset..frame_offset + plane];
        let peak = confidences
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_finite())
            .max_by(|a, b| a.1.total_cmp(b.1));
        if let Some((index, &score)) = peak {
            if score >= config.ball_floor && (0.0..=1.0).contains(&score) {
                let row = index / gw;
                let col = index % gw;
                let dx = ball_grid.data[frame_offset + plane + index];
                let dy = ball_grid.data[frame_offset + 2 * plane + index];
                let (w, h) = if let Some(sizes) = ball_size {
                    let o = frame * 2 * plane;
                    (sizes.data[o + index], sizes.data[o + plane + index])
                } else {
                    (0.012, 0.021)
                };
                if let Some(bbox) = rect(
                    (col as f32 + dx) / gw as f32,
                    (row as f32 + dy) / gh as f32,
                    w,
                    h,
                ) {
                    result.push(RawDetection {
                        frame_index: first_frame + frame as u64,
                        class_name: "ball",
                        score,
                        bbox_xyxy_norm: bbox,
                        query_index: None,
                        touch_probability: None,
                        head_point_norm: None,
                    });
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tensor(name: &str, dimensions: &[i64], data: Vec<f32>) -> Output {
        Output {
            name: name.into(),
            dimensions: dimensions.into(),
            data,
        }
    }
    #[test]
    fn ravel_decodes_player_touch_box_and_ball_grid() {
        let q = 2;
        let mut logits = vec![0.0; 9 * q * 2];
        let mut boxes = vec![0.0; 9 * q * 4];
        let mut actions = vec![-5.0; 9 * q * 2];
        let mut heads = vec![0.0; 9 * q * 2];
        for frame in 0..9 {
            let k = frame * q;
            logits[k * 2] = 0.0;
            logits[k * 2 + 1] = 2.0; // sigmoid(2)~.88
            boxes[k * 4..k * 4 + 4].copy_from_slice(&[0.5, 0.5, 0.2, 0.4]);
            actions[k * 2] = 0.8_f32.ln();
            actions[k * 2 + 1] = 0.2_f32.ln();
            heads[k * 2..k * 2 + 2].copy_from_slice(&[0.5, 0.3]);
        }
        let mut grid = vec![0.0; 9 * 3 * 2 * 2];
        let mut sizes = vec![0.0; 9 * 2 * 2 * 2];
        for frame in 0..9 {
            let o = frame * 12;
            grid[o + 1] = 0.9;
            grid[o + 4 + 1] = 0.5;
            grid[o + 8 + 1] = 0.5;
            sizes[frame * 8 + 1] = 0.1;
            sizes[frame * 8 + 4 + 1] = 0.1;
        }
        let records = decode_ravel(
            &[
                tensor("logits", &[1, 9, 2, 2], logits),
                tensor("boxes", &[1, 9, 2, 4], boxes),
                tensor("action_logits", &[1, 9, 2, 2], actions),
                tensor("head_point_norm", &[1, 9, 2, 2], heads),
                tensor("ball_grid", &[1, 9, 3, 2, 2], grid),
                tensor("ball_size", &[1, 9, 2, 2, 2], sizes),
            ],
            10,
            DecodeConfig::default(),
        )
        .unwrap();
        assert_eq!(records.len(), 18);
        assert_eq!(records[0].frame_index, 10);
        assert_eq!(records[0].query_index, Some(0));
        assert_eq!(records[0].bbox_xyxy_norm, [0.4, 0.3, 0.6, 0.7]);
        assert!((records[0].touch_probability.unwrap() - 0.8).abs() < 0.0001);
        assert_eq!(records[0].head_point_norm, Some([0.5, 0.3]));
        assert_eq!(records[1].class_name, "ball");
        assert!((records[1].score - 0.9).abs() < 1e-6);
    }
    #[test]
    fn corrupt_ravel_tensor_shape_is_not_accepted() {
        let logits = tensor("logits", &[1, 9, 2, 2], vec![0.0; 35]);
        assert!(decode_ravel(&[logits], 0, DecodeConfig::default()).is_err());
    }
    #[test]
    fn ball_heatmap_keeps_coordinate_and_normalized_extent() {
        let each = 288 * 512;
        let mut maps = vec![0.0; 18 * each];
        maps[50 * 512 + 100] = 0.9;
        maps[9 * each + 50 * 512 + 100] = 0.02;
        let records =
            decode_vball(&[tensor("heatmaps", &[1, 18, 288, 512], maps)], 0, 0.05).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].class_name, "ball");
        assert_eq!(records[0].frame_index, 0);
        assert!(records[0].bbox_xyxy_norm[2] > records[0].bbox_xyxy_norm[0]);
    }
}
