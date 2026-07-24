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
    pub(crate) fn from_monitor(monitor: &MonitorHandle) -> Rc<Self> {
        let monitor_id = monitor.id();
        let id = DisplayId::new((monitor_id as u64) ^ ((monitor_id >> 64) as u64));
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
        let fingerprint = format!(
            "{}|{}|{:?}|{:?}|{}",
            monitor.id(),
            monitor.native_id(),
            monitor.name(),
            monitor.position(),
            monitor.scale_factor()
        );

        Rc::new(Self {
            id,
            uuid: Uuid::new_v5(&Uuid::NAMESPACE_OID, fingerprint.as_bytes()),
            bounds,
        })
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
}
