use std::{
    borrow::{Borrow, BorrowMut},
    error::Error,
    fmt::{self, Display},
};

use virtio_device::{
    VirtioConfig, VirtioDevice, VirtioDeviceActions, VirtioDeviceType, VirtioMmioDevice,
};

use virtio_bindings::bindings::virtio_net;
// trait VirtioNetDevice: VirtioMmioDevice {
//     // fn test(&self) {
//     //     self.queue_select();
//     // }
// }

// impl VirtioDevice for VirtioNet {

// }

#[derive(Debug)]
pub struct VirtioNetError {}
impl Error for VirtioNetError {}
impl Display for VirtioNetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "virtio net error")
    }
}

pub struct VirtioNet {
    pub virtio_config: VirtioNetConfig,
}

pub struct VirtioNetConfig {
    pub virtio_config: VirtioConfig<virtio_queue::Queue>,
}

impl VirtioNetConfig {
    pub fn new() -> Self {
        VirtioNetConfig {
            virtio_config: VirtioConfig::new(
                virtio_net::VIRTIO_NET_F_STATUS as u64
                    | virtio_net::VIRTIO_NET_F_MAC as u64
                    | virtio_net::VIRTIO_NET_F_SPEED_DUPLEX as u64
                    | virtio_net::VIRTIO_NET_F_MTU as u64,
                vec![],
                VirtioNetConfig::config_vec(virtio_net::virtio_net_config {
                    mac: [13, 13, 13, 13, 13, 13],
                    status: 0,
                    max_virtqueue_pairs: 0,
                    mtu: 1500,
                    speed: 1000,
                    duplex: 1,
                }),
            ),
        }
    }

    fn config_vec(config: virtio_net::virtio_net_config) -> Vec<u8> {
        let mut config_vec = Vec::new();
        config_vec.extend_from_slice(&config.mac);
        config_vec.extend_from_slice(&config.status.to_le_bytes());
        config_vec.extend_from_slice(&config.max_virtqueue_pairs.to_le_bytes());
        config_vec.extend_from_slice(&config.mtu.to_le_bytes());
        config_vec.extend_from_slice(&config.speed.to_le_bytes());
        config_vec.extend_from_slice(&config.duplex.to_le_bytes());
        config_vec
    }
}

impl VirtioNet {
    pub fn new() -> Self {
        VirtioNet {
            virtio_config: VirtioNetConfig::new(),
        }
    }
}

// impl VirtioNetDevice for VirtioNet {}

impl VirtioMmioDevice for VirtioNet {}

impl Borrow<VirtioConfig<virtio_queue::Queue>> for VirtioNet {
    fn borrow(&self) -> &VirtioConfig<virtio_queue::Queue> {
        &self.virtio_config.virtio_config
    }
}

impl BorrowMut<VirtioConfig<virtio_queue::Queue>> for VirtioNet {
    fn borrow_mut(&mut self) -> &mut VirtioConfig<virtio_queue::Queue> {
        &mut self.virtio_config.virtio_config
    }
}

impl VirtioDeviceActions for VirtioNet {
    type E = VirtioNetError;

    fn activate(&mut self) -> Result<(), Self::E> {
        println!("virtio net activate");
        Ok(())
    }
    fn reset(&mut self) -> std::result::Result<(), Self::E> {
        println!("virtio net reset");
        Ok(())
    }
}

impl VirtioDeviceType for VirtioNet {
    fn device_type(&self) -> u32 {
        virtio_bindings::bindings::virtio_net::VIRTIO_ID_NET
    }
}
