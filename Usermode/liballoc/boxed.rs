
use core::ptr::Unique;
use core::marker::Unsize;
use core::ops::CoerceUnsized;

#[lang = "owned_box"]
pub struct Box<T: ?Sized>( Unique<T> );

impl<T: ?Sized + Unsize<U>, U: ?Sized> CoerceUnsized<Box<U>> for Box<T> { }

impl<T> Box<T>
{
	pub fn new(v: T) -> Box<T> {
		box v
	}
}

impl<T: ?Sized> Box<T>
{
	pub fn into_raw(this: Self) -> *mut T {
		// SAFE: Box<T> and *mut T have the same repr
		unsafe {
			::core::mem::transmute(this)
		}
	}
}

