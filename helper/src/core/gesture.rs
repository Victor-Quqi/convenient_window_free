use crate::config::{GestureMode, GesturePoint, GestureTemplate, MouseGestureConfig};
use crate::platform::{Point, Rect};

const SAMPLE_COUNT: usize = 64;

/// Keep the whole stroke within a bounded buffer, including its release point.
pub fn append_gesture_point(points: &mut Vec<Point>, point: Point, release: bool) {
    if let Some(last) = points.last() {
        let distance = distance_i32(*last, point);
        if distance == 0.0 || (!release && distance < 2.0) {
            return;
        }
    }
    if points.len() >= 512 {
        let mut tolerance = 0.5;
        loop {
            let simplified = simplify_stroke(points, tolerance);
            if simplified.len() <= 256 {
                *points = simplified;
                break;
            }
            tolerance *= 2.0;
        }
    }
    points.push(point);
}

fn simplify_stroke(points: &[Point], tolerance: f32) -> Vec<Point> {
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut pending = vec![(0, points.len() - 1)];
    while let Some((start, end)) = pending.pop() {
        let a = points[start];
        let b = points[end];
        let dx = b.x as f32 - a.x as f32;
        let dy = b.y as f32 - a.y as f32;
        let length_squared = dx * dx + dy * dy;
        let mut farthest = None;
        let mut maximum = tolerance * tolerance;
        for (index, p) in points.iter().enumerate().take(end).skip(start + 1) {
            let px = p.x as f32 - a.x as f32;
            let py = p.y as f32 - a.y as f32;
            let t = if length_squared > 0.0 {
                ((px * dx + py * dy) / length_squared).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let distance = (px - t * dx).powi(2) + (py - t * dy).powi(2);
            if distance > maximum {
                maximum = distance;
                farthest = Some(index);
            }
        }
        if let Some(index) = farthest {
            keep[index] = true;
            pending.push((start, index));
            pending.push((index, end));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(*point))
        .collect()
}

#[derive(Clone, Debug)]
pub struct GestureMatch<'a> {
    pub gesture: &'a GestureTemplate,
    pub score: f32,
    pub region: Option<Rect>,
}

pub fn recognize<'a>(points: &[Point], config: &'a MouseGestureConfig) -> Option<GestureMatch<'a>> {
    if points.len() < 2 || path_length_pixels(points) < config.min_distance as f32 {
        return None;
    }

    if let Some(region) = rectangle_region(points) {
        if let Some(gesture) = config
            .gestures
            .iter()
            .find(|gesture| gesture.enabled && gesture.mode == GestureMode::RegionScreenshot)
        {
            return Some(GestureMatch {
                gesture,
                score: 1.0,
                region: Some(region),
            });
        }
    }

    if is_circle(points) {
        if let Some(gesture) = config
            .gestures
            .iter()
            .find(|gesture| gesture.enabled && gesture.id == "gesture-circle")
        {
            return Some(GestureMatch {
                gesture,
                score: 0.98,
                region: None,
            });
        }
    }

    let candidate_points = points
        .iter()
        .map(|point| GesturePoint {
            x: point.x as f32,
            y: point.y as f32,
        })
        .collect::<Vec<_>>();
    let candidate = normalize(&candidate_points)?;
    let candidate_efficiency = path_efficiency(&candidate_points);
    let mut best: Option<GestureMatch<'a>> = None;
    for gesture in config
        .gestures
        .iter()
        .filter(|gesture| gesture.enabled && gesture.mode == GestureMode::Action)
    {
        for sample in &gesture.samples {
            let Some(template) = normalize(sample) else {
                continue;
            };
            let distance = candidate
                .iter()
                .zip(template.iter())
                .map(|(left, right)| {
                    ((left.x - right.x).powi(2) + (left.y - right.y).powi(2)).sqrt()
                })
                .sum::<f32>()
                / SAMPLE_COUNT as f32;
            let efficiency_penalty = (candidate_efficiency - path_efficiency(sample)).abs() * 0.45;
            let score =
                (1.0 - distance / std::f32::consts::SQRT_2 - efficiency_penalty).clamp(0.0, 1.0);
            if best.as_ref().is_none_or(|current| score > current.score) {
                best = Some(GestureMatch {
                    gesture,
                    score,
                    region: None,
                });
            }
        }
    }
    // UI sensitivity maps to a deliberately conservative similarity floor. The
    // default 72 therefore requires 0.86, leaving unknown strokes cancelled.
    let threshold = 0.5 + config.sensitivity as f32 / 200.0;
    best.filter(|result| result.score >= threshold)
}

pub fn path_length_pixels(points: &[Point]) -> f32 {
    points
        .windows(2)
        .map(|pair| distance_i32(pair[0], pair[1]))
        .sum()
}

pub fn rectangle_region(points: &[Point]) -> Option<Rect> {
    if points.len() < 5 {
        return None;
    }
    let left = points.iter().map(|point| point.x).min()?;
    let top = points.iter().map(|point| point.y).min()?;
    let right = points.iter().map(|point| point.x).max()?;
    let bottom = points.iter().map(|point| point.y).max()?;
    let width = right - left;
    let height = bottom - top;
    if width < 40 || height < 40 {
        return None;
    }
    let diagonal = ((width * width + height * height) as f32).sqrt();
    if distance_i32(points[0], *points.last()?) > diagonal * 0.24 {
        return None;
    }
    let perimeter = (2 * (width + height)) as f32;
    let length = path_length_pixels(points);
    if !(perimeter * 0.72..=perimeter * 1.45).contains(&length) {
        return None;
    }

    let corner_radius = diagonal * 0.14;
    let corners = [
        Point { x: left, y: top },
        Point { x: right, y: top },
        Point {
            x: right,
            y: bottom,
        },
        Point { x: left, y: bottom },
    ];
    if !corners.iter().all(|corner| {
        points
            .iter()
            .any(|point| distance_i32(*corner, *point) <= corner_radius)
    }) {
        return None;
    }

    Some(Rect {
        left,
        top,
        right: right + 1,
        bottom: bottom + 1,
    })
}

fn is_circle(points: &[Point]) -> bool {
    if points.len() < 8 {
        return false;
    }
    let left = points.iter().map(|point| point.x).min().unwrap_or_default();
    let top = points.iter().map(|point| point.y).min().unwrap_or_default();
    let right = points.iter().map(|point| point.x).max().unwrap_or_default();
    let bottom = points.iter().map(|point| point.y).max().unwrap_or_default();
    let width = (right - left) as f32;
    let height = (bottom - top) as f32;
    if width < 36.0 || height < 36.0 || width / height < 0.62 || width / height > 1.62 {
        return false;
    }
    let diagonal = (width * width + height * height).sqrt();
    if distance_i32(points[0], *points.last().unwrap_or(&points[0])) > diagonal * 0.28 {
        return false;
    }
    let expected = std::f32::consts::PI * (width + height) / 2.0;
    let ratio = path_length_pixels(points) / expected.max(1.0);
    (0.82..=1.18).contains(&ratio)
}

fn normalize(points: &[GesturePoint]) -> Option<Vec<GesturePoint>> {
    let resampled = resample(points, SAMPLE_COUNT)?;
    let min_x = resampled
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let max_x = resampled
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = resampled
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_y = resampled
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let scale = (max_x - min_x).max(max_y - min_y);
    if !scale.is_finite() || scale <= f32::EPSILON {
        return None;
    }
    let center_x = resampled.iter().map(|point| point.x).sum::<f32>() / resampled.len() as f32;
    let center_y = resampled.iter().map(|point| point.y).sum::<f32>() / resampled.len() as f32;
    Some(
        resampled
            .into_iter()
            .map(|point| GesturePoint {
                x: (point.x - center_x) / scale,
                y: (point.y - center_y) / scale,
            })
            .collect(),
    )
}

fn resample(points: &[GesturePoint], count: usize) -> Option<Vec<GesturePoint>> {
    if points.len() < 2
        || points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return None;
    }
    let lengths: Vec<f32> = points
        .windows(2)
        .map(|pair| ((pair[1].x - pair[0].x).powi(2) + (pair[1].y - pair[0].y).powi(2)).sqrt())
        .collect();
    let total: f32 = lengths.iter().sum();
    if total <= f32::EPSILON {
        return None;
    }
    let mut output = Vec::with_capacity(count);
    let mut segment = 0usize;
    let mut traversed = 0.0;
    for index in 0..count {
        let target = total * index as f32 / (count - 1) as f32;
        while segment + 1 < points.len() - 1 && traversed + lengths[segment] < target {
            traversed += lengths[segment];
            segment += 1;
        }
        let segment_length = lengths[segment].max(f32::EPSILON);
        let ratio = ((target - traversed) / segment_length).clamp(0.0, 1.0);
        output.push(GesturePoint {
            x: points[segment].x + (points[segment + 1].x - points[segment].x) * ratio,
            y: points[segment].y + (points[segment + 1].y - points[segment].y) * ratio,
        });
    }
    Some(output)
}

fn distance_i32(left: Point, right: Point) -> f32 {
    let dx = (left.x - right.x) as f32;
    let dy = (left.y - right.y) as f32;
    (dx * dx + dy * dy).sqrt()
}

fn path_efficiency(points: &[GesturePoint]) -> f32 {
    let Some((first, rest)) = points.split_first() else {
        return 0.0;
    };
    let Some(last) = rest.last() else { return 0.0 };
    let displacement = ((last.x - first.x).powi(2) + (last.y - first.y).powi(2)).sqrt();
    let length = points
        .windows(2)
        .map(|pair| ((pair[1].x - pair[0].x).powi(2) + (pair[1].y - pair[0].y).powi(2)).sqrt())
        .sum::<f32>();
    if length <= f32::EPSILON {
        0.0
    } else {
        displacement / length
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MouseGestureConfig;

    fn captured(vertices: &[Point], step: i32) -> Vec<Point> {
        let mut points = Vec::new();
        for pair in vertices.windows(2) {
            let length = distance_i32(pair[0], pair[1]).ceil() as i32;
            for offset in (0..length).step_by(step as usize) {
                let t = offset as f32 / length as f32;
                append_gesture_point(
                    &mut points,
                    Point {
                        x: (pair[0].x as f32 + (pair[1].x - pair[0].x) as f32 * t).round() as i32,
                        y: (pair[0].y as f32 + (pair[1].y - pair[0].y) as f32 * t).round() as i32,
                    },
                    false,
                );
                assert!(points.len() <= 512);
            }
        }
        append_gesture_point(&mut points, *vertices.last().unwrap(), true);
        assert_eq!(points.first(), vertices.first());
        assert_eq!(points.last(), vertices.last());
        points
    }

    #[test]
    fn dense_and_sparse_long_strokes_keep_the_same_gesture() {
        let config = MouseGestureConfig::default();
        let rectangle = [
            Point { x: -300, y: 0 },
            Point { x: 0, y: 0 },
            Point { x: 0, y: 300 },
            Point { x: -300, y: 300 },
            Point { x: -300, y: 0 },
        ];
        let l = [
            Point { x: 0, y: 0 },
            Point { x: 0, y: 1200 },
            Point { x: 1040, y: 1200 },
        ];
        let circle: Vec<_> = (0..=1200)
            .map(|i| {
                let angle = std::f32::consts::TAU * i as f32 / 1200.0;
                Point {
                    x: (angle.cos() * 600.0).round() as i32,
                    y: (angle.sin() * 600.0).round() as i32,
                }
            })
            .collect();
        for (vertices, expected) in [
            (&rectangle[..], "gesture-rectangle"),
            (&l[..], "gesture-l"),
            (&circle[..], "gesture-circle"),
        ] {
            for step in [1, 2, 10] {
                let points = captured(vertices, step);
                assert_eq!(recognize(&points, &config).unwrap().gesture.id, expected);
            }
        }
    }

    #[test]
    fn repeated_compaction_preserves_turns_and_release_endpoint() {
        let vertices = [
            Point { x: -12000, y: -100 },
            Point {
                x: -12000,
                y: 12000,
            },
            Point { x: 1, y: 12000 },
        ];
        let points = captured(&vertices, 1);
        assert!(points.contains(&vertices[1]));
        assert_eq!(
            recognize(&points, &MouseGestureConfig::default())
                .unwrap()
                .gesture
                .id,
            "gesture-l"
        );
        let mut points = vec![Point { x: 0, y: 0 }];
        append_gesture_point(&mut points, Point { x: 1, y: 0 }, false);
        assert_eq!(points.len(), 1);
        append_gesture_point(&mut points, Point { x: 1, y: 0 }, true);
        assert_eq!(points.last(), Some(&Point { x: 1, y: 0 }));
    }

    #[test]
    fn directional_templates_keep_their_direction() {
        let config = MouseGestureConfig::default();
        let up = [
            Point { x: 400, y: 500 },
            Point { x: 402, y: 390 },
            Point { x: 399, y: 260 },
        ];
        let down = [
            Point { x: 400, y: 260 },
            Point { x: 398, y: 390 },
            Point { x: 401, y: 500 },
        ];
        assert_eq!(recognize(&up, &config).unwrap().gesture.id, "gesture-up");
        assert_eq!(
            recognize(&down, &config).unwrap().gesture.id,
            "gesture-down"
        );
    }

    #[test]
    fn short_or_unknown_strokes_do_not_execute() {
        let config = MouseGestureConfig::default();
        assert!(recognize(&[Point { x: 0, y: 0 }, Point { x: 4, y: 3 }], &config).is_none());
        let zigzag = [
            Point { x: 0, y: 0 },
            Point { x: 100, y: 10 },
            Point { x: 0, y: 20 },
            Point { x: 100, y: 30 },
            Point { x: 0, y: 40 },
        ];
        assert!(recognize(&zigzag, &config).is_none());
        let downward_zigzag = [
            Point { x: 620, y: 400 },
            Point { x: 760, y: 420 },
            Point { x: 620, y: 440 },
            Point { x: 760, y: 460 },
            Point { x: 620, y: 480 },
        ];
        assert!(recognize(&downward_zigzag, &config).is_none());
    }

    #[test]
    fn closed_rectangle_returns_signed_screen_bounds() {
        let points = [
            Point { x: -800, y: 100 },
            Point { x: -200, y: 102 },
            Point { x: -198, y: 500 },
            Point { x: -802, y: 498 },
            Point { x: -800, y: 100 },
        ];
        assert_eq!(
            rectangle_region(&points),
            Some(Rect {
                left: -802,
                top: 100,
                right: -197,
                bottom: 501
            })
        );
        let config = MouseGestureConfig::default();
        let result = recognize(&points, &config).unwrap();
        assert_eq!(result.gesture.mode, GestureMode::RegionScreenshot);
        assert!(result.region.is_some());
    }

    #[test]
    fn a_circle_is_not_misclassified_as_a_rectangle() {
        let points: Vec<_> = (0..=40)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / 40.0;
                Point {
                    x: (300.0 + angle.cos() * 100.0) as i32,
                    y: (300.0 + angle.sin() * 100.0) as i32,
                }
            })
            .collect();
        assert!(rectangle_region(&points).is_none());
        assert_eq!(
            recognize(&points, &MouseGestureConfig::default())
                .unwrap()
                .gesture
                .id,
            "gesture-circle"
        );
    }
}
