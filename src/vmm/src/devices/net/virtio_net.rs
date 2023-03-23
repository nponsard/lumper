use std::{
    borrow::{Borrow, BorrowMut},
    cmp,
    error::Error,
    fmt::{self, Debug, Display},
    io::Write,
    os::fd::{AsRawFd, RawFd},
    sync::atomic::Ordering,
};

use virtio_device::{VirtioConfig, VirtioDeviceActions, VirtioDeviceType, VirtioMmioDevice};

use virtio_bindings::bindings::{
    virtio_blk::VIRTIO_F_VERSION_1,
    virtio_net::{
        self, virtio_net_hdr_v1, VIRTIO_NET_F_CSUM, VIRTIO_NET_F_GUEST_CSUM,
        VIRTIO_NET_F_GUEST_TSO4, VIRTIO_NET_F_GUEST_TSO6, VIRTIO_NET_F_GUEST_UFO,
        VIRTIO_NET_F_HOST_TSO4, VIRTIO_NET_F_HOST_TSO6, VIRTIO_NET_F_HOST_UFO,
    },
};
use virtio_queue::{Queue, QueueOwnedT, QueueT};
use vm_device::{MutVirtioMmioDevice, VirtioMmioOffset};
use vm_memory::{Bytes, GuestAddress, GuestAddressSpace};
use vmm_sys_util::eventfd::EventFd;

use crate::devices::net::bindings;

use super::tap::Tap;

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
    pub tap: Tap,
}

impl<M: GuestAddressSpace + Clone + Send> VirtioNet<M> {
    pub fn new(memory: M, irq_fd: EventFd) -> Self {
        Self {
            device_config: VirtioConfig::new(
                (1 << VIRTIO_F_VERSION_1)
                    | (1 << 29_u64)
                    | (1 << 35_u64)
                    | (1 << VIRTIO_NET_F_CSUM)
                    | (1 << VIRTIO_NET_F_GUEST_CSUM)
                    | (1 << VIRTIO_NET_F_GUEST_TSO4)
                    | (1 << VIRTIO_NET_F_GUEST_TSO6)
                    | (1 << VIRTIO_NET_F_GUEST_UFO)
                    | (1 << VIRTIO_NET_F_HOST_TSO4)
                    | (1 << VIRTIO_NET_F_HOST_TSO6)
                    | (1 << VIRTIO_NET_F_HOST_UFO),
                vec![Queue::new(256).unwrap(), Queue::new(256).unwrap()],
                Self::config_vec(virtio_net::virtio_net_config {
                    mac: [13, 13, 13, 13, 13, 13],
                    status: 0,
                    max_virtqueue_pairs: 1,
                    mtu: 1420,
                    speed: 1000,
                    duplex: 1,
                }),
            ),
            address_space: memory,
            irq_fd,
            tap: Tap::open_named("tap1").unwrap(),
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

    pub fn tap_raw_fd(&self) -> RawFd {
        self.tap.as_raw_fd()
    }

    fn write_frame_to_guest(
        &mut self,
        original_buffer: &mut [u8; 65565],
        size: usize,
    ) -> Result<bool, VirtioNetError> {
        let mem = self.address_space.memory();
        let mut chain = match &mut self.device_config.queues[0].iter(&*mem).unwrap().next() {
            Some(c) => c.to_owned(),
            _ => return Ok(false),
        };

        let mut count = 0;
        let buffer = &mut original_buffer[..size];

        while let Some(desc) = chain.next() {
            let left = buffer.len() - count;

            // println!(
            //     "left: {}, buffer_len {}, desc_len: {}, count: {}, size: {}",
            //     left,
            //     buffer.len(),
            //     desc.len(),
            //     count,
            //     size
            // );

            if left == 0 {
                break;
            }

            // print nicely what we are writing
            // let mut s = String::new();
            // for i in 0..cmp::min(left, desc.len() as usize) {
            //     s.push_str(&format!("{:02x} ", buffer[count + i]));
            // }
            // println!("writing to guest: {}", s);

            let len = cmp::min(left, desc.len() as usize);
            chain
                .memory()
                .write_slice(&buffer[count..count + len], desc.addr())
                .unwrap();

            count += len;
        }

        if count != buffer.len() {
            // The frame was too large for the chain.
            println!("rx frame too large");
        }

        self.device_config.queues[0]
            .add_used(&*mem, chain.head_index(), count as u32)
            .unwrap();

        println!("adding used buffer to queue");

        Ok(true)
    }

    pub fn process_tap(&mut self) -> Result<(), VirtioNetError> {
        use std::io::Read;
        let mut something_read = false;

        {
            let buffer = &mut [0u8; 65565];

            loop {
                let mut read_size = 0;
                read_size += match self.tap.read(&mut buffer[read_size..]) {
                    Ok(size) => size,
                    Err(_) => {
                        // TODO: Do something (logs, metrics, etc.) in response to an error when
                        // reading from tap. EAGAIN means there's nothing available to read anymore
                        // (because we open the TAP as non-blocking).
                        break;
                    }
                };

                something_read = true;

                let mem = self.address_space.memory().borrow_mut().clone();

                println!("read {} bytes from tap", read_size);

                if !self.write_frame_to_guest(buffer, read_size)?
                    && !self.device_config.queues[0]
                        .enable_notification(&*mem.clone())
                        .unwrap()
                {
                    break;
                }
            }
        }

        if something_read {
            println!("trying to notify guest");
            if self.device_config.queues[0]
                .needs_notification(&*self.address_space.memory())
                .unwrap()
            {
                self.device_config
                    .interrupt_status
                    .store(1, Ordering::SeqCst);
                println!("notifying guest");
                let irq = &mut self.irq_fd;
                irq.write(1).unwrap();
            }
        }

        Ok(())
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioDeviceType for VirtioNet<M> {
    fn device_type(&self) -> u32 {
        virtio_net::VIRTIO_ID_NET
    }
}

impl<M: GuestAddressSpace + Clone + Send> VirtioMmioDevice for VirtioNet<M> {
    fn queue_notify(&mut self, val: u32) {
        if val == 0 {
            return self.process_tap().unwrap();
        }

        let mem = self.address_space.memory().clone();
        let irq = &mut self.irq_fd;
        let queue = &mut self.device_config.queues[1];

        loop {
            queue.disable_notification(&*mem).unwrap();

            // Consume entries from the available ring.
            while let Some(chain) = queue.iter(&*mem).unwrap().next() {
                chain.clone().for_each(|desc| {
                    if (desc.len() as usize) < VIRTIO_HDR_LEN {
                        println!("invalid virtio header length");
                        return;
                    }

                    // let mut header_buffer: [u8; VIRTIO_HDR_LEN] = [0u8; VIRTIO_HDR_LEN];
                    let mut data_buffer: Vec<u8> = Vec::new();

                    // Safe since we checked the length of the data
                    // mem.read_slice(&mut header_buffer, desc.addr()).unwrap();
                    // let header = virtio_net_hdr_v1 {
                    //     flags: header_buffer[0],
                    //     gso_type: header_buffer[1],
                    //     hdr_len: u16::from_le_bytes([header_buffer[2], header_buffer[3]]),
                    //     gso_size: u16::from_le_bytes([header_buffer[4], header_buffer[5]]),
                    //     csum_start: u16::from_le_bytes([header_buffer[6], header_buffer[7]]),
                    //     csum_offset: u16::from_le_bytes([header_buffer[8], header_buffer[9]]),
                    //     num_buffers: u16::from_le_bytes([header_buffer[10], header_buffer[11]]),
                    // };
                    data_buffer.resize(desc.len() as usize, 0u8);
                    mem.read_slice(&mut data_buffer, desc.addr()).unwrap();
                    self.tap.write(&data_buffer);
                    // if (desc.len() as usize) > VIRTIO_HDR_LEN {
                    // data_buffer.drain(..VIRTIO_HDR_LEN);
                    // }
                });

                queue.add_used(&*mem, chain.head_index(), 0x100).unwrap();

                if queue.needs_notification(&*mem).unwrap() {
                    irq.write(1).unwrap();
                }
            }

            if !queue.enable_notification(&*mem).unwrap() {
                break;
            }
        }

        self.process_tap().unwrap();
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
        self.tap.set_vnet_hdr_size(VIRTIO_HDR_LEN as i32).unwrap();

        // Set offload flags to match the relevant virtio features of the device (for now,
        // statically set in the constructor.
        self.tap
            .set_offload(
                bindings::TUN_F_CSUM
                    | bindings::TUN_F_UFO
                    | bindings::TUN_F_TSO4
                    | bindings::TUN_F_TSO6,
            )
            .unwrap();

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
            "sent {} for {}",
            u64::from(offset),
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
