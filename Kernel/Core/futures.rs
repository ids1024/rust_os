// "Tifflin" Kernel
// - By John Hodge (thePowersGang)
//
// Core/futures.rs
//! Helpers for standard library futures/async

use crate::sync::EventChannel;
use core::sync::atomic::{AtomicUsize,Ordering};
use core::task;

pub fn msleep(ms: usize) -> impl core::future::Future<Output=()> {
	struct Sleep(u64);
	impl core::future::Future for Sleep {
		type Output = ();
		fn poll(self: core::pin::Pin<&mut Self>, _cx: &mut task::Context) -> task::Poll<Self::Output> {
			if self.0 < crate::time::ticks() {
				todo!("msleep - {} < {}", self.0, crate::time::ticks());
			}
			else {
				task::Poll::Ready( () )
			}
		}
	}
	Sleep(crate::time::ticks() + ms as u64)
}

/// Create a waker handle that does nothing
pub fn null_waker() -> task::Waker
{
	fn rw_clone(_: *const ()) -> task::RawWaker {
		task::RawWaker::new(1 as *const (), &VTABLE)
	}
	fn rw_wake(_: *const ()) {
	}
	fn rw_wake_by_ref(_: *const ()) {
	}
	fn rw_drop(_: *const ()) {
	}
	static VTABLE: task::RawWakerVTable = task::RawWakerVTable::new(
		rw_clone, rw_wake, rw_wake_by_ref, rw_drop,
		);
	// SAFE: This waker does nothing
	unsafe {
		task::Waker::from_raw(rw_clone(1 as *const ()))
	}
}

/// Simple async task executor
pub fn runner(mut f: impl FnMut(&mut task::Context))
{
	let waiter = SimpleWaiter::new();

	// SAFE: The inner waker above won't move
	let waker = unsafe { task::Waker::from_raw(waiter.raw_waker()) };
	let mut context = task::Context::from_waker(&waker);

	loop
	{
		f(&mut context);
		waiter.sleep();
	}
}

struct SimpleWaiter
{
	ref_count: AtomicUsize,
	ec: EventChannel,
}

impl SimpleWaiter
{
	fn new() -> SimpleWaiter {
		SimpleWaiter {
			ref_count: Default::default(),
			ec: Default::default(),
		}
	}

	fn sleep(&self) {
		self.ec.sleep();
	}

	fn raw_waker(&self) -> task::RawWaker {
		static VTABLE: task::RawWakerVTable = task::RawWakerVTable::new(
			/*clone:*/ SimpleWaiter::rw_clone,
			/*wake:*/ SimpleWaiter::rw_wake,
			/*wake_by_ref:*/ SimpleWaiter::rw_wake_by_ref,
			/*drop:*/ SimpleWaiter::rw_drop,
			);
		self.ref_count.fetch_add(1, Ordering::SeqCst);
		task::RawWaker::new(self as *const _ as *const (), &VTABLE)
	}
	unsafe fn raw_self(raw_self: &*const ()) -> &Self {
		&*(*raw_self as *const Self)
	}
	unsafe fn rw_clone(raw_self: *const ()) -> task::RawWaker {
		Self::raw_self(&raw_self).raw_waker()
	}
	unsafe fn rw_wake(raw_self: *const ()) {
		Self::rw_wake_by_ref(raw_self);
		Self::rw_drop(raw_self);
	}
	unsafe fn rw_wake_by_ref(raw_self: *const ()) {
		// Poke sleeping thread
		Self::raw_self(&raw_self).ec.post();
	}
	unsafe fn rw_drop(raw_self: *const ()) {
		// Decrement reference count
		Self::raw_self(&raw_self).ref_count.fetch_sub(1, Ordering::SeqCst);
	}
}
impl core::ops::Drop for SimpleWaiter {
	fn drop(&mut self) {
		assert!(*self.ref_count.get_mut() == 0, "References left when waker dropped");
	}
}
