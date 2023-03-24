use vm_allocator::{AddressAllocator, NodeState, RangeInclusive};

/// Dedicated [`Result`](https://doc.rust-lang.org/std/result/) type.
pub type Result<T> = std::result::Result<T, vm_allocator::Error>;

// 48-bit address space
const MAX_48BIT_ADDRESS: u64 = 0x0000_ffff_ffff_ffff;
const BASE_ADDRESS: u64 = 0x0000_0000_0000_0000;

const APIC_END: u64 = 1 << 32; // end of 32-bit address space
const APIC_SIZE: u64 = 0x1400000; // 20 MB

const APIC_START: u64 = APIC_END - APIC_SIZE; // 1 MB

// Start address for the EBDA (Extended Bios Data Area). Older computers (like the one this VMM
// emulates) typically use 1 KiB for the EBDA, starting at 0x9fc00.
// See https://wiki.osdev.org/Memory_Map_(x86) for more information.
const EBDA_START: u64 = 0x0009_fc00;
pub const HIMEM_START: u64 = 0x0010_0000; // 1 MB

pub const DEFAULT_ADDRESSS_ALIGNEMNT: u64 = 4;

pub trait LumperMemoryAllocator {
    fn new_64_bit_memory_allocator() -> Result<AddressAllocator>;
    fn register_x86_reserved_regions(&mut self) -> Result<()>;
}

impl LumperMemoryAllocator for AddressAllocator {
    fn new_64_bit_memory_allocator() -> Result<AddressAllocator> {
        Ok(AddressAllocator::new(
            BASE_ADDRESS,
            MAX_48BIT_ADDRESS - BASE_ADDRESS,
        )?)
    }
    fn register_x86_reserved_regions(&mut self) -> Result<()> {
        // // Add an entry for EBDA
        let ebda_range = RangeInclusive::new(EBDA_START, HIMEM_START)?;
        println!("EBDA range: {:?}", ebda_range);
        self.allocate_range(ebda_range, NodeState::ReservedMapped)?;

        // Add an entry for APIC, BIOS, etc
        let apic_range = RangeInclusive::new(APIC_START, APIC_END)?;

        println!("APIC range: {:?}", apic_range);
        self.allocate_range(apic_range, NodeState::ReservedNotMapped)?;
        Ok(())
    }
}
