use std::{
    borrow::{Borrow, BorrowMut},
    error::Error,
    fmt::{self, Debug, Display},
};

use virtio_device::{VirtioConfig, VirtioDeviceActions, VirtioDeviceType, VirtioMmioDevice};

use virtio_bindings::bindings::virtio_net;
use virtio_device::VirtioDevice;
use virtio_queue::QueueT;
use vm_device::{
    MutVirtioMmioDevice, VirtioMmioDevice as VirtioMmioDeviceVmDevice, VirtioMmioOffset,
};
use vm_memory::{GuestAddress, GuestAddressSpace};
use vmm_sys_util::eventfd::EventFd;
#[derive(Debug)]
pub enum VirtioNetError {
    RegisterError,
}
impl Error for VirtioNetError {}
impl Display for VirtioNetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "virtio net error")
    }
}

pub struct VirtioNet<M: GuestAddressSpace + Clone + Send> {
    pub device_config: VirtioNetConfig,
    pub address_space: M,
    pub irq_fd: EventFd,
}

pub struct VirtioNetConfig {
    pub guest_page_size: u16,
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
            guest_page_size: 0,
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

impl<M: GuestAddressSpace + Clone + Send> VirtioNet<M> {
    pub fn new(memory: M, irq_fd: EventFd) -> Self {
        VirtioNet {
            device_config: VirtioNetConfig::new(),
            address_space: memory,
            irq_fd,
        }
    }

    fn is_reading_register(&self, offset: &VirtioMmioOffset) -> bool {
        if let VirtioMmioOffset::DeviceSpecific(offset) = offset {
            !(*offset as usize) < self.device_config.virtio_config.config_space.len() * 8
        } else {
            true
        }
    }

    fn register_write(
        &mut self,
        offset: VirtioMmioOffset,
        data: &[u8],
    ) -> Result<(), VirtioNetError> {
        match offset {
            VirtioMmioOffset::HostFeaturesSel(_) => {
                self.device_config.virtio_config.device_features_select = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                Ok(())
            }

            VirtioMmioOffset::GuestFeatures(_) => {
                let mut features = u64::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                if self.device_config.virtio_config.driver_features_select != 0 {
                    features >>= 32;
                }
                self.device_config.virtio_config.driver_features = features;
                Ok(())
            }

            VirtioMmioOffset::GuestFeaturesSel(_) => {
                self.device_config.virtio_config.driver_features_select = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                Ok(())
            }

            VirtioMmioOffset::GuestPageSize(_) => {
                self.device_config.guest_page_size = u16::from_le_bytes(
                    data[0..2]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                Ok(())
            }

            VirtioMmioOffset::QueueSel(_) => {
                self.device_config.virtio_config.queue_select = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                ) as u16;
                Ok(())
            }

            VirtioMmioOffset::QueueNum(_) => {
                self.device_config.virtio_config.queues
                    [self.device_config.virtio_config.queue_select as usize]
                    .set_size(u16::from_le_bytes(
                        data[0..2]
                            .try_into()
                            .map_err(|_| VirtioNetError::RegisterError)?,
                    ));
                Ok(())
            }

            VirtioMmioOffset::QueueAlign(_) => {
                println!("Ignoring queue alignment");
                Ok(())
            }

            // Since its a 64 bit register, there is probably 2 writes to this ?
            VirtioMmioOffset::QueuePfn(_) => {
                let queue_pfn = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                self.device_config.virtio_config.queues
                    [self.device_config.virtio_config.queue_select as usize]
                    .set_desc_table_address(
                        Some(queue_pfn * self.device_config.guest_page_size as u32),
                        None,
                    );
                Ok(())
            }

            VirtioMmioOffset::QueueNotify(_) => {
                let queue_notify = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                println!("Queue notify: {}", queue_notify);
                Ok(())
            }

            VirtioMmioOffset::InterruptAck(_) => {
                let interrupt_ack = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                println!("Interrupt ack: {}", interrupt_ack);
                Ok(())
            }

            VirtioMmioOffset::Status(_) => {
                let status = u32::from_le_bytes(
                    data[0..4]
                        .try_into()
                        .map_err(|_| VirtioNetError::RegisterError)?,
                );
                println!("Status: {}", status);
                Ok(())
            }

            _ => Err(VirtioNetError::RegisterError),
        }
    }

    fn register_read(
        &self,
        field: VirtioMmioOffset,
        data: &mut [u8],
    ) -> Result<(), VirtioNetError> {
        match field {
            VirtioMmioOffset::MagicValue(_) => {
                let magic_value = 0x74726976_u32.to_le_bytes();
                data.copy_from_slice(&magic_value);
                Ok(())
            }
            VirtioMmioOffset::VirtioVersion(_) => {
                let virtio_version = 0x2_u32.to_le_bytes();
                data.copy_from_slice(&virtio_version);
                Ok(())
            }
            VirtioMmioOffset::DeviceId(_) => {
                let device_id = 0x1_u32.to_le_bytes();
                data.copy_from_slice(&device_id);
                Ok(())
            }
            VirtioMmioOffset::VendorId(_) => {
                let vendor_id = 0x1AF4_u32.to_le_bytes();
                data.copy_from_slice(&vendor_id);
                Ok(())
            }
            VirtioMmioOffset::HostFeatures(_) => {
                let mut device_features = self.device_config.virtio_config.device_features;
                if self.device_config.virtio_config.device_features_select != 0 {
                    device_features >>= 32;
                }
                data.copy_from_slice(&(device_features as u32).to_le_bytes());
                Ok(())
            }
            VirtioMmioOffset::QueueNumMax(_) => {
                let queue_num_max = 0x1000 as u32;
                data.copy_from_slice(&queue_num_max.to_le_bytes());
                Ok(())
            }
            VirtioMmioOffset::QueuePfn(_) => {
                let queue_pfn = 0x12341234 as u32;
                data.copy_from_slice(&queue_pfn.to_le_bytes());
                Ok(())
            }
            VirtioMmioOffset::InterruptStatus(_) => {
                let interrupt_status = 0x0 as u32;
                self.interrupt_status()
                    .load(std::sync::atomic::Ordering::Relaxed) as u32;
                data.copy_from_slice(&interrupt_status.to_le_bytes());
                Ok(())
            }
            VirtioMmioOffset::Status(_) => {
                let status = self.device_config.virtio_config.device_status as u32;
                data.copy_from_slice(&status.to_le_bytes());
                Ok(())
            }
            VirtioMmioOffset::DeviceSpecific(offset) => {
                self.read_config(offset as usize, data);
                Ok(())
            }
            _ => Err(VirtioNetError::RegisterError),
        }
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioDeviceType for VirtioNet<M> {
    fn device_type(&self) -> u32 {
        virtio_net::VIRTIO_ID_NET
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioMmioDevice for VirtioNet<M> {}

impl<M: GuestAddressSpace + Clone + Send> Borrow<VirtioConfig<virtio_queue::Queue>>
    for VirtioNet<M>
{
    fn borrow(&self) -> &VirtioConfig<virtio_queue::Queue> {
        &self.device_config.virtio_config
    }
}

impl<M: GuestAddressSpace + Clone + Send> BorrowMut<VirtioConfig<virtio_queue::Queue>>
    for VirtioNet<M>
{
    fn borrow_mut(&mut self) -> &mut VirtioConfig<virtio_queue::Queue> {
        &mut self.device_config.virtio_config
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioDeviceActions for VirtioNet<M> {
    type E = VirtioNetError;

    fn activate(&mut self) -> Result<(), Self::E> {
        println!("virtio net activate");
        panic!("virtio net activate");
        Ok(())
    }
    fn reset(&mut self) -> std::result::Result<(), Self::E> {
        println!("virtio net reset");
        panic!("virtio net activate");
        Ok(())
    }
}

impl<M: GuestAddressSpace + Clone + Send> MutVirtioMmioDevice for VirtioNet<M> {
    fn virtio_mmio_read(&mut self, base: GuestAddress, offset: VirtioMmioOffset, data: &mut [u8]) {
        if self.is_reading_register(&offset) {
            let registerer_field = VirtioMmioOffset::from(offset);

            if let Err(e) = self.register_read(registerer_field, data) {
                println!("virtio net mmio read error: {:?}", e);
            }
        }
        println!(
            "sent {}",
            Vec::from(data)
                .iter()
                .map(|x| format!("{:02x}", x))
                .collect::<String>()
        );
        return;
    }

    fn virtio_mmio_write(&mut self, base: GuestAddress, offset: VirtioMmioOffset, data: &[u8]) {
        if self.is_reading_register(&offset) {
            let registerer_field = VirtioMmioOffset::from(offset);

            if let Err(e) = self.register_write(registerer_field, data) {
                println!("virtio net mmio read error: {:?}", e);
            }
        }
        println!(
            "sent {}",
            Vec::from(data)
                .iter()
                .map(|x| format!("{:02x}", x))
                .collect::<String>()
        );
        return;
    }
}
