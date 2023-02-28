use std::{
    borrow::{Borrow, BorrowMut},
    error::Error,
    fmt::{self, Debug, Display},
};

use virtio_device::{
    VirtioConfig, VirtioDevice, VirtioDeviceActions, VirtioDeviceType, VirtioMmioDevice,
};

use virtio_bindings::bindings::{
    virtio_blk::VIRTIO_F_VERSION_1,
    virtio_net::{self, virtio_net_hdr_v1},
};
use virtio_queue::{Queue, QueueOwnedT, QueueT};
use vm_device::{MutVirtioMmioDevice, VirtioMmioOffset};
use vm_memory::{Address, Bytes, GuestAddress, GuestAddressSpace};
use vmm_sys_util::eventfd::EventFd;

const VIRTIO_HDR_LEN: usize = ::core::mem::size_of::<virtio_net_hdr_v1>();

#[derive(Debug)]

pub enum VirtioNetError {}
impl Error for VirtioNetError {}
impl Display for VirtioNetError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "virtio net error")
    }
}

pub struct VirtioNet<M: GuestAddressSpace + Clone + Send> {
    pub device_config: VirtioConfig<Queue>,
    pub address_space: M,
    pub irq_fd: EventFd,
}

impl<M: GuestAddressSpace + Clone + Send> VirtioNet<M> {
    pub fn new(memory: M, irq_fd: EventFd) -> Self {
        Self {
            device_config: VirtioConfig::new(
                1 << VIRTIO_F_VERSION_1,
                vec![Queue::new(256).unwrap(), Queue::new(256).unwrap()],
                Self::config_vec(virtio_net::virtio_net_config {
                    mac: [13, 13, 13, 13, 13, 13],
                    status: 0,
                    max_virtqueue_pairs: 1,
                    mtu: 1500,
                    speed: 1000,
                    duplex: 1,
                }),
            ),
            address_space: memory,
            irq_fd,
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

    fn is_reading_register(&self, offset: &VirtioMmioOffset) -> bool {
        if let VirtioMmioOffset::DeviceSpecific(offset) = offset {
            !(*offset as usize) < self.device_config.config_space.len() * 8
        } else {
            true
        }
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioDeviceType for VirtioNet<M> {
    fn device_type(&self) -> u32 {
        virtio_net::VIRTIO_ID_NET
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioMmioDevice for VirtioNet<M> {
    fn queue_notify(&mut self, val: u32) {
        println!("queue notify");
        let mem = self.address_space.memory().clone();
        let queue = self.queue_mut(val as u16).unwrap();

        queue.iter(mem.clone()).unwrap().for_each(|desc| {
            desc.for_each(|desc| {
                if (desc.len() as usize) < VIRTIO_HDR_LEN {
                    println!("invalid virtio header length");
                    return;
                }

                let mut header_buffer: [u8; VIRTIO_HDR_LEN] = [0u8; VIRTIO_HDR_LEN];
                let mut data_buffer: Vec<u8> = Vec::new();

                // Safe since we checked the length of the data
                mem.read_slice(&mut header_buffer, desc.addr()).unwrap();
                let header = virtio_net_hdr_v1 {
                    flags: header_buffer[0],
                    gso_type: header_buffer[1],
                    hdr_len: u16::from_le_bytes([header_buffer[2], header_buffer[3]]),
                    gso_size: u16::from_le_bytes([header_buffer[4], header_buffer[5]]),
                    csum_start: u16::from_le_bytes([header_buffer[6], header_buffer[7]]),
                    csum_offset: u16::from_le_bytes([header_buffer[8], header_buffer[9]]),
                    num_buffers: u16::from_le_bytes([header_buffer[10], header_buffer[11]]),
                };
                data_buffer.resize(desc.len() as usize - VIRTIO_HDR_LEN, 0u8);
                mem.read_slice(
                    &mut data_buffer,
                    desc.addr().overflowing_add(VIRTIO_HDR_LEN as u64).0,
                )
                .unwrap();

                println!("header: {:?}", header);
                println!("data: ");

                data_buffer.iter().for_each(|b| print!("{:#x?}", *b));
            })
        });
    }
}

impl<M: GuestAddressSpace + Clone + Send> Borrow<VirtioConfig<virtio_queue::Queue>>
    for VirtioNet<M>
{
    fn borrow(&self) -> &VirtioConfig<virtio_queue::Queue> {
        &self.device_config
    }
}

impl<M: GuestAddressSpace + Clone + Send> BorrowMut<VirtioConfig<virtio_queue::Queue>>
    for VirtioNet<M>
{
    fn borrow_mut(&mut self) -> &mut VirtioConfig<virtio_queue::Queue> {
        &mut self.device_config
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioDeviceActions for VirtioNet<M> {
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

impl<M: GuestAddressSpace + Clone + Send> MutVirtioMmioDevice for VirtioNet<M> {
    fn virtio_mmio_read(&mut self, _base: GuestAddress, offset: VirtioMmioOffset, data: &mut [u8]) {
        if self.is_reading_register(&offset) {
            self.read(u64::from(offset), data);
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

    fn virtio_mmio_write(&mut self, _base: GuestAddress, offset: VirtioMmioOffset, data: &[u8]) {
        if self.is_reading_register(&offset) {
            self.write(u64::from(offset), data);
        }
        return;
    }
}
