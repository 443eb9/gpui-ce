use std::rc::Rc;

use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, point, size};
use uuid::Uuid;
use winit::monitor::MonitorHandle;

#[derive(Debug)]
pub(crate) struct WinitDisplay {
    id: DisplayId,
    uuid: Uuid,
    bounds: Bounds<Pixels>,
}

impl WinitDisplay {
    pub(crate) fn from_monitor(monitor: &MonitorHandle, id: DisplayId) -> Rc<Self> {
        let scale_factor = monitor.scale_factor() as f32;
        let position = monitor.position().unwrap_or_default();
        let physical_size = monitor
            .current_video_mode()
            .map(|mode| mode.size())
            .unwrap_or_default();
        let bounds = Bounds::new(
            point(
                gpui::px(position.x as f32 / scale_factor),
                gpui::px(position.y as f32 / scale_factor),
            ),
            size(
                gpui::px(physical_size.width as f32 / scale_factor),
                gpui::px(physical_size.height as f32 / scale_factor),
            ),
        );

        Rc::new(Self {
            id,
            uuid: Self::uuid_for_monitor(monitor),
            bounds,
        })
    }

    pub(crate) fn uuid_for_monitor(monitor: &MonitorHandle) -> Uuid {
        let mut fingerprint = Vec::new();
        if let Some(name) = monitor.name().filter(|name| !name.is_empty()) {
            fingerprint.extend_from_slice(name.as_bytes());
        } else {
            fingerprint.extend_from_slice(&monitor.native_id().to_le_bytes());
            if let Some(position) = monitor.position() {
                fingerprint.extend_from_slice(&position.x.to_le_bytes());
                fingerprint.extend_from_slice(&position.y.to_le_bytes());
            }
            if let Some(mode) = monitor.current_video_mode() {
                let size = mode.size();
                fingerprint.extend_from_slice(&size.width.to_le_bytes());
                fingerprint.extend_from_slice(&size.height.to_le_bytes());
            }
            fingerprint.extend_from_slice(&monitor.scale_factor().to_bits().to_le_bytes());
        }
        Uuid::new_v5(&Uuid::NAMESPACE_OID, &fingerprint)
    }

    pub(crate) fn matches_monitor(&self, monitor: &MonitorHandle) -> bool {
        self.uuid == Self::uuid_for_monitor(monitor)
    }
}

impl PlatformDisplay for WinitDisplay {
    fn id(&self) -> DisplayId {
        self.id
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    fn visible_bounds(&self) -> Bounds<Pixels> {
        // TODO(winit): Monitor work areas are not exposed by winit.
        self.bounds
    }
}
