use super::Geometry;

const DEFAULT_WIDTH: u32 = 420;
const DEFAULT_HEIGHT: u32 = 420;
const SCREEN_MARGIN: i32 = 24;
const MIN_VISIBLE_WIDTH: i64 = 64;
const MIN_VISIBLE_HEIGHT: i64 = 48;

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorBounds {
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

pub fn recover_geometry(
    saved: Option<&Geometry>,
    monitors: &[MonitorBounds],
    min_width: u32,
    min_height: u32,
) -> Option<Geometry> {
    if monitors.is_empty() {
        return None;
    }

    let target = saved
        .and_then(|geometry| monitor_for_saved_geometry(geometry, monitors))
        .unwrap_or_else(|| primary_monitor(monitors));

    let (requested_width, requested_height) = match saved {
        Some(geometry) if geometry.scale_factor.is_finite() && geometry.scale_factor > 0.0 => {
            let scale_ratio = target.scale_factor / geometry.scale_factor;
            (
                (f64::from(geometry.width) * scale_ratio).round() as u32,
                (f64::from(geometry.height) * scale_ratio).round() as u32,
            )
        }
        Some(geometry) => (geometry.width, geometry.height),
        None => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
    };

    let width = clamp_dimension(requested_width, min_width, target.width);
    let height = clamp_dimension(requested_height, min_height, target.height);

    let saved_is_visible = saved
        .map(|geometry| sufficiently_visible(geometry, target))
        .unwrap_or(false);
    let (x, y) = if let Some(geometry) = saved.filter(|_| saved_is_visible) {
        clamp_position(geometry.x, geometry.y, width, height, target)
    } else {
        default_position(width, height, target)
    };

    Some(Geometry {
        x,
        y,
        width,
        height,
        scale_factor: target.scale_factor,
        monitor_name: target.name.clone(),
    })
}

fn monitor_for_saved_geometry<'a>(
    geometry: &Geometry,
    monitors: &'a [MonitorBounds],
) -> Option<&'a MonitorBounds> {
    if let Some(name) = geometry.monitor_name.as_ref() {
        if let Some(monitor) = monitors
            .iter()
            .find(|monitor| monitor.name.as_ref() == Some(name))
        {
            return Some(monitor);
        }
    }

    monitors
        .iter()
        .max_by_key(|monitor| intersection_area(geometry, monitor))
        .filter(|monitor| intersection_area(geometry, monitor) > 0)
}

fn primary_monitor(monitors: &[MonitorBounds]) -> &MonitorBounds {
    monitors
        .iter()
        .find(|monitor| monitor.is_primary)
        .unwrap_or(&monitors[0])
}

fn clamp_dimension(requested: u32, minimum: u32, available: u32) -> u32 {
    requested.max(minimum.min(available)).min(available)
}

fn clamp_position(x: i32, y: i32, width: u32, height: u32, monitor: &MonitorBounds) -> (i32, i32) {
    let max_x = i64::from(monitor.x) + i64::from(monitor.width) - i64::from(width);
    let max_y = i64::from(monitor.y) + i64::from(monitor.height) - i64::from(height);
    (
        i64::from(x).clamp(i64::from(monitor.x), max_x) as i32,
        i64::from(y).clamp(i64::from(monitor.y), max_y) as i32,
    )
}

fn default_position(width: u32, height: u32, monitor: &MonitorBounds) -> (i32, i32) {
    let right = i64::from(monitor.x) + i64::from(monitor.width);
    let preferred_x = right - i64::from(width) - i64::from(SCREEN_MARGIN);
    let preferred_y = i64::from(monitor.y) + i64::from(SCREEN_MARGIN);
    clamp_position(
        preferred_x as i32,
        preferred_y as i32,
        width,
        height,
        monitor,
    )
}

fn sufficiently_visible(geometry: &Geometry, monitor: &MonitorBounds) -> bool {
    let left = i64::from(geometry.x).max(i64::from(monitor.x));
    let top = i64::from(geometry.y).max(i64::from(monitor.y));
    let right = (i64::from(geometry.x) + i64::from(geometry.width))
        .min(i64::from(monitor.x) + i64::from(monitor.width));
    let bottom = (i64::from(geometry.y) + i64::from(geometry.height))
        .min(i64::from(monitor.y) + i64::from(monitor.height));

    right - left >= MIN_VISIBLE_WIDTH && bottom - top >= MIN_VISIBLE_HEIGHT
}

fn intersection_area(geometry: &Geometry, monitor: &MonitorBounds) -> i64 {
    let left = i64::from(geometry.x).max(i64::from(monitor.x));
    let top = i64::from(geometry.y).max(i64::from(monitor.y));
    let right = (i64::from(geometry.x) + i64::from(geometry.width))
        .min(i64::from(monitor.x) + i64::from(monitor.width));
    let bottom = (i64::from(geometry.y) + i64::from(geometry.height))
        .min(i64::from(monitor.y) + i64::from(monitor.height));
    (right - left).max(0) * (bottom - top).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primary() -> MonitorBounds {
        MonitorBounds {
            name: Some("Primary".into()),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
            is_primary: true,
        }
    }

    #[test]
    fn places_new_widget_near_primary_top_right() {
        let geometry = recover_geometry(None, &[primary()], 320, 180).unwrap();
        assert_eq!(geometry.x, 1920 - 420 - SCREEN_MARGIN);
        assert_eq!(geometry.y, SCREEN_MARGIN);
        assert_eq!((geometry.width, geometry.height), (420, 420));
    }

    #[test]
    fn recovers_fully_offscreen_geometry_to_primary() {
        let saved = Geometry {
            x: 8000,
            y: 8000,
            width: 420,
            height: 220,
            scale_factor: 1.0,
            monitor_name: Some("Disconnected".into()),
        };
        let geometry = recover_geometry(Some(&saved), &[primary()], 320, 180).unwrap();
        assert_eq!(geometry.monitor_name.as_deref(), Some("Primary"));
        assert!(geometry.x >= 0 && geometry.x + geometry.width as i32 <= 1920);
        assert!(geometry.y >= 0 && geometry.y + geometry.height as i32 <= 1080);
    }

    #[test]
    fn clamps_partially_visible_geometry_onto_its_monitor() {
        let saved = Geometry {
            x: -100,
            y: 50,
            width: 500,
            height: 300,
            scale_factor: 1.0,
            monitor_name: Some("Primary".into()),
        };
        let geometry = recover_geometry(Some(&saved), &[primary()], 320, 180).unwrap();
        assert_eq!((geometry.x, geometry.y), (0, 50));
        assert_eq!((geometry.width, geometry.height), (500, 300));
    }

    #[test]
    fn rescales_saved_logical_size_for_new_monitor_scale() {
        let mut monitor = primary();
        monitor.scale_factor = 2.0;
        let saved = Geometry {
            x: 100,
            y: 100,
            width: 420,
            height: 220,
            scale_factor: 1.0,
            monitor_name: Some("Primary".into()),
        };
        let geometry = recover_geometry(Some(&saved), &[monitor], 320, 180).unwrap();
        assert_eq!((geometry.width, geometry.height), (840, 440));
        assert_eq!(geometry.scale_factor, 2.0);
    }
}
