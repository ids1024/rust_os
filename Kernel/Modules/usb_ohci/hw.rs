//
//!
use core::sync::atomic::{Ordering};

#[repr(usize)]
#[allow(dead_code)]
#[derive(Debug)]
pub enum Regs
{
	HcRevision = 0,
	HcControl,
	HcCommandStatus,
	HcInterruptStatus,
	HcInterruptEnable,
	HcInterruptDisable,

	HcHCCA,
	HcPeriodCurrentED,
	HcControlHeadED,
	HcControlCurrentED,
	HcBulkHeadED,
	HcBulkCurrentED,
	HcDoneHead,

	HcFmInterval,
	HcFmRemaining,
	HcFmNumber,
	HcPeriodicStart,
	HcLSThreshold,

	//  0: 7 = NDP (Max of 15)
	HcRhDescriptorA,
	HcRhDescriptorB,
	HcRhStatus,
	HcRhPortStatus0,
	HcRhPortStatus1,
	HcRhPortStatus2,
	HcRhPortStatus3,
	HcRhPortStatus4,
	HcRhPortStatus5,
	HcRhPortStatus6,
	HcRhPortStatus7,
	HcRhPortStatus8,
	HcRhPortStatus9,
	HcRhPortStatus10,
	HcRhPortStatus11,
	HcRhPortStatus12,
	HcRhPortStatus13,
	HcRhPortStatus14,
	HcRhPortStatus15,
}

pub const HCCMDSTATUS_HCR: u32 = 1 << 0;	// "HostControllerReset"
pub const HCCMDSTATUS_CLF: u32 = 1 << 1;	// "ControlListFilled"
pub const HCCMDSTATUS_BLF: u32 = 1 << 1;	// "BulkListFilled"
pub const HCCMDSTATUS_OCR: u32 = 1 << 1;	// "OwnershipChangeRequest"

// Host Controller Communication Area
// 256 bytes total
#[repr(C)]
pub struct Hcca
{
	pub interrupt_table: [u32; 128 / 4],
	pub frame_number: u16,
	_pad1: u16,
	pub done_head: u32,
	_reserved: [u32; 116 / 4],
}

// Size: 16 bytes, fits 256 per page - i.e just enough for a u8
#[repr(C)]
pub struct Endpoint
{
	//  0: 6 = Address
	//  7:10 = Endpoint Num
	// 11:12 = Direction (TD, OUT, IN, TD)
	// 13    = Speed (Full, Low)
	// 14    = Skip entry
	// 15    = Format (Others, Isochronous)
	// 16:26 = Max Packet Size
	// 27:30 = AVAIL
	// 31    = AVAIL
	pub flags: u32,
	//  0: 3 = AVAIL
	//  4:31 = TailP
	pub tail_ptr: u32,
	//  0    = Halted (Queue stopped due to error)
	//  1    = Data toggle carry
	//  2: 3 = ZERO
	//  4:31 = HeadP
	pub head_ptr: u32,
	//  0: 3 = AVAIL
	//  4:31 = NextED
	pub next_ed: u32,	// Next endpoint descriptor in the chain.

	// TODO: Extra metadata?
}
impl Endpoint
{
	/// (AVAIL) Lock bit
	pub const FLAG_LOCKED: u32 = (1 << 31);
	/// (AVAIL) Allocated bit
	pub const FLAG_ALLOC: u32 = (1 << 30);

	pub fn atomic_flags(s: *const Self) -> *const core::sync::atomic::AtomicU32 {
		// NOTE: flags is the first field
		s as *const core::sync::atomic::AtomicU32
	}
}

/// A general (non-isochronous) transfer descriptor
#[repr(C)]
pub struct GeneralTD
{
	/// Flags
	//  0:17 = AVAIL
	//       > 0: Allocated bit (1 when allocated)
	//       > 1: Auto-free (release once complete)
	//       > 2: Complete
	// 18    = Buffer Rounding (Allow an undersized packet)
	// 19:20 = Direction (SETUP, OUT, IN, Resvd)
	// 21:23 = Delay Interrupt (Frame count, 7 = no int)
	// 24:25 = Data Toggle (ToggleCarry, ToggleCarry, 0, 1)
	// 26:27 = Error Count
	// 28:31 = Condition Code
	flags: ::core::sync::atomic::AtomicU32,

	// Base address of packet (or current when being read)
	// - NOTE: This updates if IN didn't use all of the buffer
	cbp: u32,

	/// Next transfer descriptor in the chain
	next_td: u32,

	/// Address of final byte in buffer
	// - Note, this can be in a different page to the base address to a maximum of two
	buffer_end: u32,

	// -- Metadata
	meta_async_waker: ::core::cell::UnsafeCell<[u64; 2]>,
}
impl GeneralTD
{
	pub const FLAG_ALLOCATED: u32 = 1 << 0;
	pub const FLAG_INIT: u32 = 1 << 1;
	pub const FLAG_AUTOFREE: u32 = 1 << 2;
	pub const FLAG_COMPLETE: u32 = 1 << 3;
	pub const FLAG_LOCKED: u32 = 1 << 4;
	//pub const FLAG_ROUNDING: u32 = 1 << 18;

	pub fn maybe_alloc(&self) -> bool
	{
		self.flags.compare_and_swap(0, Self::FLAG_ALLOCATED, Ordering::SeqCst) == 0
	}
	/// UNSAFE: Addresses in `first_byte`, `last_byte`, and `next_td` are passed to hardware
	pub unsafe fn init(s: *mut Self, flags: u32, first_byte: u32, last_byte: u32, next_td: u32, waker: ::core::task::Waker)
	{
		// If the flags are just allocated, then this is not initialised (so should be unique)
		assert!( (*s).flags.load(Ordering::SeqCst) == Self::FLAG_ALLOCATED );
		(*s).flags.store(flags | Self::FLAG_ALLOCATED | Self::FLAG_INIT, Ordering::SeqCst);
		::core::ptr::write(&mut (*s).cbp, first_byte);
		::core::ptr::write(&mut (*s).buffer_end, last_byte);
		::core::ptr::write(&mut (*s).next_td, next_td);
		// - Store the (single pointer) async handle in the 64-bit meta field
		::core::ptr::write((*s).meta_async_waker.get() as *mut _, waker);
	}
	pub fn mark_free(&self)
	{
		assert!(self.flags.load(Ordering::SeqCst) & Self::FLAG_INIT != 0);
		let _lh = self.take_waker_lock();
		self.flags.store(Self::FLAG_LOCKED, Ordering::SeqCst);
	}
	pub fn mark_complete(&self) -> bool
	{
		assert!(self.flags.load(Ordering::SeqCst) & Self::FLAG_INIT != 0);
		self.flags.fetch_or(Self::FLAG_COMPLETE, Ordering::SeqCst) & Self::FLAG_AUTOFREE != 0
	}
	pub fn get_next(&self) -> u32
	{
		assert!(self.flags.load(Ordering::Acquire) & Self::FLAG_INIT != 0);
		self.next_td
	}
	/// Returns `Some(unused_space)`
	pub fn is_complete(&self) -> Option<usize>
	{
		assert!(self.flags.load(Ordering::Acquire) & Self::FLAG_INIT != 0);
		if self.flags.load(Ordering::SeqCst) & Self::FLAG_COMPLETE != 0
		{
			let cbp = self.cbp;
			let end = self.buffer_end;

			log_debug!("is_complete({:#x}): {:#x} -- {:#x}", ::kernel::memory::virt::get_phys(self), cbp, end);
			if cbp == 0 {	// When complete, zero is written to CBP
				Some( 0 )
			}
			// Same page, simple subtraction
			else if cbp & !0xFFF == end & !0xFFF {
				Some( (end - cbp) as usize + 1 )
			}
			// Different page
			else {
				let rem1 = 0x1000 - (cbp & 0xFFF);
				let rem2 = end & 0xFFF;
				//log_trace!("is_complete: {:#x} + {:#x}", rem1, rem2);
				Some( (rem1 + rem2) as usize + 1 )
			}
		}
		else
		{
			None
		}
	}

	fn take_waker_lock(&self) -> GeneralTdLockedWaker
	{
		let int_lh = ::kernel::arch::sync::hold_interrupts();
		loop
		{
			let flags = self.flags.load(Ordering::SeqCst) & !Self::FLAG_LOCKED;
			if self.flags.compare_and_swap(flags, flags | Self::FLAG_LOCKED, Ordering::Acquire) == flags
			{
				return GeneralTdLockedWaker {
					flags: &self.flags,
					// SAFE: Access controlled via the above locks
					waker: unsafe { &mut *(self.meta_async_waker.get() as *mut ::core::task::Waker) },
					_ints: int_lh,
					};
			}
		}
	}
	pub fn take_waker(&self) -> ::core::task::Waker
	{
		let mut waker_lh = self.take_waker_lock();
		::core::mem::replace(&mut waker_lh.waker, ::kernel::futures::null_waker())
	}
	pub fn update_waker(&self, waker: &::core::task::Waker)
	{
		let waker_lh = self.take_waker_lock();
		if !waker_lh.waker.will_wake(waker)
		{
			*waker_lh.waker = waker.clone();
		}
	}
}
struct GeneralTdLockedWaker<'a>
{
	flags: &'a ::core::sync::atomic::AtomicU32,
	waker: &'a mut ::core::task::Waker,
	_ints: ::kernel::arch::sync::HeldInterrupts,
}
impl<'a> ::core::ops::Drop for GeneralTdLockedWaker<'a>
{
	fn drop(&mut self) {
		assert!( self.flags.fetch_and(!GeneralTD::FLAG_LOCKED, Ordering::Release) & GeneralTD::FLAG_LOCKED != 0 );
	}
}

// 32 * 16  = 512 bytes long
/// Structure of part of the HCCA (but NOT specified by the hardware, just suggested)
pub struct IntLists
{
	/// 16ms polling periods
	pub int_16ms: [Endpoint; 16],
	pub int_8ms: [Endpoint; 8],
	pub int_4ms: [Endpoint; 4],
	pub int_2ms: [Endpoint; 2],
	pub int_1ms: [Endpoint; 1],
	/// The end of any list
	pub stop_endpoint: Endpoint,
}


