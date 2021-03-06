// 
//! USB Core
#![no_std]
#![feature(linkage)]	// for module_define!
#![feature(try_blocks)]
use kernel::prelude::*;
use kernel::lib::mem::aref::{Aref,ArefBorrow};
use kernel::sync::Mutex;

#[macro_use]
extern crate kernel;
extern crate stack_dst;
extern crate utf16;

module_define!{usb_core, [], init}

fn init()
{
}

mod hub;
pub mod host;
pub mod device;
pub mod handle;
mod hw_decls;

/// Reference to a hub
#[derive(Clone)]
enum HubRef
{
	//Root(ArefBorrow<Host>),
	Root(HostRef),
	Device(ArefBorrow<HubDevice>),
}

#[derive(Clone)]
struct HostRef(*const Host);
unsafe impl Send for HostRef where Host: Sync {
}
unsafe impl Sync for HostRef where Host: Sync {
}
impl core::ops::Deref for HostRef {
	type Target = Host;
	fn deref(&self) -> &Host {
		// SAFE: TODO - Enforce safety
		unsafe { &*self.0 }
	}
}

#[derive(Default)]
struct AddressPool
{
	next_id: u8,
	used_ids: [u8; 128/8],
}
/// Representation of a host/bus
/// - Used to hold the device address allocation logic/structures
struct Host
{
	driver: Box<dyn host::HostController>,
	addresses: Mutex<AddressPool>,

	//// If true, EP0 is currently being enumerated
	//endpoint_zero_state: bool,
	endpoint_zero_handle: ControlEndpoint,
	
	//root_ports: OnceCell<Vec<Port>>,
	root_ports: Vec<PortState>,
	
	//device_workers: [Mutex<Option<core::pin::Pin<Box<dyn core::future::Future<Output=()> + Send>>>>; 255],
	device_workers: Vec< Mutex<Option<core::pin::Pin<Box<dyn core::future::Future<Output=()> + Send>>>> >,
}
struct HubDevice
{
	host: ArefBorrow<Host>,
	int_ep: host::Handle<dyn host::InterruptEndpoint>,
}
struct HostEnt
{
	host: Aref<Host>,
	_worker: kernel::threads::WorkerThread,
}

static HOST_LIST: Mutex<Vec<HostEnt>> = ::kernel::sync::Mutex::new(Vec::new_const());

/// Add a new host controller/bus to the system
pub fn register_host(mut driver: Box<dyn host::HostController>, nports: u8)
{
	let host = Aref::new(Host {
		addresses: ::kernel::sync::Mutex::new(AddressPool {
			next_id: 1,
			used_ids: [0; 128/8],
			}),
		endpoint_zero_handle: ControlEndpoint {
			inner: driver.init_control(crate::host::EndpointAddr::new(0, 0), 64),
			},
		root_ports: {
			let mut v = Vec::new();
			v.resize_with(nports as usize, || PortState::new());
			v
			},
		//device_workers: Default::default(),
		device_workers: {
			let mut v = Vec::new();
			v.resize_with(255, Default::default);
			v
			},

		driver: driver,
		});

	let hb = host.borrow();
	let mut lh = HOST_LIST.lock();
	lh.push(HostEnt {
		host,
		_worker: ::kernel::threads::WorkerThread::new("USB Host", move || host_worker(hb)),
		});
}

fn host_worker(host: ArefBorrow<Host>)
{
	let mut host_async = host.root_event_task();
	// SAFE: Not moved until it's dropped
	let mut host_async = unsafe { core::pin::Pin::new_unchecked(&mut host_async) };
	kernel::futures::runner(|context| {
		use core::future::Future;
		match host_async.as_mut().poll(context)
		{
		core::task::Poll::Ready(v) => panic!("Host root task completed"),
		core::task::Poll::Pending => {},
		}
		// Have a list of port workers
		// TODO: A hub updating might lead t a new entry being added here
		for p in host.device_workers.iter()
		{
			let mut p = p.lock();
			let done = if let Some(ref mut p) = *p
				{
					p.as_mut().poll(context).is_ready()
				}
				else
				{
					false
				};
			if done {
				log_debug!("Device worker complete");
				*p = None;
			}
		}
	});
}

impl HubRef
{
	fn host(&self) -> &Host {
		match self
		{
		&HubRef::Root  (ref h) => h,
		&HubRef::Device(ref h) => todo!("get host - hub"),
		}
	}

	async fn set_port_feature(&self, port_idx: usize, feat: host::PortFeature)
	{
		match self
		{
		&HubRef::Root  (ref h) => h.driver.set_port_feature(port_idx, feat),
		&HubRef::Device(ref h) => h.set_port_feature(port_idx, feat).await,
		}
	}
	async fn clear_port_feature(&self, port_idx: usize, feat: host::PortFeature)
	{
		match self
		{
		&HubRef::Root  (ref h) => h.driver.clear_port_feature(port_idx, feat),
		&HubRef::Device(ref h) => h.clear_port_feature(port_idx, feat).await,
		}
	}
	async fn get_port_feature(&self, port_idx: usize, feat: host::PortFeature) -> bool
	{
		match self
		{
		&HubRef::Root  (ref h) => h.driver.get_port_feature(port_idx, feat),
		&HubRef::Device(ref h) => h.get_port_feature(port_idx, feat).await,
		}
	}
}

struct PortState
{
}
impl PortState
{
	fn new() -> Self {
		PortState {
		}
	}

	fn signal_connected(&self, hub: HubRef, port_idx: u8)
	{
		hub.clone().host().add_device(move |addr| PortDev::new(hub, port_idx, addr).worker());
	}
}
struct PortDev
{
	hub: HubRef,
	port_idx: u8,
	addr: u8,
}
impl PortDev
{
	pub fn new(hub: HubRef, port_idx: u8, addr: u8) -> PortDev {
		PortDev {
			hub,
			port_idx,
			addr,
			}
	}
	fn host(&self) -> &Host {
		self.hub.host()
	}

	fn set_port_feature(&self, feat: host::PortFeature) -> impl core::future::Future<Output=()> + '_ {
		self.hub.set_port_feature(self.port_idx as usize, feat)
	}
	fn clear_port_feature(&self, feat: host::PortFeature) -> impl core::future::Future<Output=()> + '_ {
		self.hub.clear_port_feature(self.port_idx as usize, feat)
	}
	fn get_port_feature(&self, feat: host::PortFeature) -> impl core::future::Future<Output=bool> + '_ {
		self.hub.get_port_feature(self.port_idx as usize, feat)
	}
	//async fn getclear_port_feature(&self, feat: host::PortFeature) -> bool {
	//	let rv = self.get_port_feature(feat).await;
	//	if rv {
	//		self.clear_port_feature(feat).await;
	//	}
	//	rv
	//}

	async fn initialise_port(&self, address: u8)
	{
		let addr0_handle = self.host().get_address_zero().await;
		if ! self.get_port_feature(host::PortFeature::Power).await
		{
			todo!("Power on a newly connected port");
		}
		self.set_port_feature(host::PortFeature::Reset).await;
		kernel::futures::msleep(50).await;
		self.clear_port_feature(host::PortFeature::Reset).await;
		kernel::futures::msleep(2).await;
		self.set_port_feature(host::PortFeature::Enable).await;
		addr0_handle.send_setup_address(address).await;
	}

	async fn enumerate(&self, ep0: ControlEndpoint) -> Result<Vec<Interface>, &'static str>
	{
		let dev_descr: hw_decls::Descriptor_Device = ep0.read_descriptor(/*index*/0).await?;
		log_debug!("dev_descr = {:?}", dev_descr);
		log_debug!("dev_descr.usb_version = {:x}", dev_descr.usb_version);
		log_debug!("dev_descr.vendor_id/device_id = {:04x}:{:04}", dev_descr.vendor_id, dev_descr.device_id);
		let mfg_str = ep0.read_string(dev_descr.manufacturer_str).await?;
		let prod_str = ep0.read_string(dev_descr.product_str).await?;
		let ser_str = ep0.read_string(dev_descr.serial_number_str).await?;
		log_debug!("dev_descr.manufacturer_str = #{} {}", dev_descr.manufacturer_str, mfg_str);
		log_debug!("dev_descr.product_str = #{} {}", dev_descr.product_str, prod_str);
		log_debug!("dev_descr.serial_number_str = #{} {}", dev_descr.serial_number_str, ser_str);

		// Enumerate all configurations
		for idx in 0 .. dev_descr.num_configurations
		{
			let base_cfg: hw_decls::Descriptor_Configuration = ep0.read_descriptor(idx).await?;
			let cfg_str = ep0.read_string(base_cfg.configuration_str).await?;
			log_debug!("cfg[{}] = {:?} ({:?})", idx, cfg_str, base_cfg);
		}

		if dev_descr.num_configurations > 1 {
			// TODO: Pick an alternative configuration (if there's more than 1)
			// - Pick the first one that finds a driver?
		}

		// Just hard-code configuration 0 for now
		self.set_configuration(ep0, 0).await
	}

	async fn set_configuration(&self, ep0: ControlEndpoint, idx: u8) -> Result<Vec<Interface>, &'static str>
	{
		// Get the base configuration descriptor
		let base_cfg: hw_decls::Descriptor_Configuration = ep0.read_descriptor(idx).await?;
		// - Fetch the full descriptor (includes interfaces and endpoints)
		let mut cfg_buf = vec![0; base_cfg.total_length as usize];
		ep0.read_descriptor_raw(<hw_decls::Descriptor_Configuration as hw_decls::Descriptor>::TYPE, idx, &mut cfg_buf).await?;
		let other_descriptors = &cfg_buf[base_cfg.length as usize..];

		// Count the number of interfaces and pre-allocate the return list
		let n_ints = hw_decls::IterDescriptors(other_descriptors)
			.filter(|v| is!(v, Ok(hw_decls::DescriptorAny::Interface(..))))
			.count();
		let mut interfaces = Vec::with_capacity(n_ints);

		// Iterate descriptors, looking for interfaces
		// - Tracks the previous interface and the start of the intervening descriptor list
		let mut it = hw_decls::IterDescriptors(other_descriptors);
		let mut last_int: Option<(hw_decls::Descriptor_Interface, &[u8],)> = None;
		while let Some(desc) = it.next()
		{
			if let Ok(hw_decls::DescriptorAny::Interface(v)) = desc
			{
				let s = ep0.read_string(v.interface_str).await?;
				log_debug!("Interface string '{}'", s);
				if let Some( (v,start) ) = last_int.take()
				{
					// Note: minus 9 so it excludes the current iteration's interface
					let endpoint_list = &start[..start.len() - it.0.len() - 9];
					interfaces.push( self.spawn_interface(&ep0, &v, endpoint_list) );
				}
				last_int = Some( (v, it.0) );
			}
		}
		if let Some( (v,start) ) = last_int.take()
		{
			let endpoint_list = &start[..start.len() - it.0.len()];
			interfaces.push( self.spawn_interface(&ep0, &v, endpoint_list) );
		}
		Ok(interfaces)
	}

	fn spawn_interface(&self, endpoint_0: &ControlEndpoint, int_desc: &hw_decls::Descriptor_Interface, descriptors: &[u8]) -> Interface
	{
		let full_class
			= (int_desc.interface_class as u32) << 16
			| (int_desc.interface_sub_class as u32) << 8
			| (int_desc.interface_protocol as u32) << 0
			;
		// - Look up using the interface class specs
		//  > May also want specialised drivers?
		// - If a driver can't be found, what do?

		// Idea:
		// - Each interface is constructed as-is according to the descriptors
		// - Store the interfaces in `self` (or return from `enumerate`)
		// - Assign a driver to the constructed interface
		let mut endpts = Vec::with_capacity(int_desc.num_endpoints as usize);
		for desc in hw_decls::IterDescriptors(descriptors)
		{
			if let Ok(hw_decls::DescriptorAny::Endpoint(ep_desc)) = desc
			{
				let ep_num = ep_desc.address & 0xF;
				let ep_dir_in = (ep_desc.address & 0x80 != 0);
				let ep_type = (ep_desc.attributes & 0x3) >> 0;
				let max_packet_size = (ep_desc.max_packet_size.0 as u16) | (ep_desc.max_packet_size.1 as u16 & 0x03) << 8;
				let poll_period = ep_desc.max_polling_interval;
				log_debug!("EP {} {} {} MPS={}",
					ep_num,
					["OUT","IN"][ep_dir_in as usize],
					["Control","Isoch","Bulk","Interrupt"][ep_type as usize],
					max_packet_size,
					);
				endpts.push(match ep_type
					{
					0 => Endpoint::Control(ControlEndpoint::new(self.host(), self.addr, ep_num, max_packet_size as usize)),
					1 => todo!("Isoch endpoint"),//Endpoint::Isoch(IsochEndpoint::new(self.host(), self.addr, ep_num, max_packet_size, ep_dir_in, ...)),
					2 => Endpoint::Bulk(BulkEndpoint::new(self.host(), self.addr, ep_num, ep_dir_in, max_packet_size as usize)),
					3 => if ep_dir_in {
							Endpoint::Interrupt(InterruptEndpoint::new(self.host(), self.addr, ep_num, max_packet_size as usize, poll_period as usize))
						}
						else {
							todo!("Out interrupt endpoint?");
						},
					_ => unreachable!("endpoint type"),
					});
			}
		}

		// Locate a suitable driver
		match crate::device::find_driver(0,0, full_class)
		{
		Some(d) => {
			// Start the device
			Interface::Bound(d.start_device(endpts, descriptors).into())
			},
		None => {
			use ::kernel::lib::borrow::ToOwned;;
			log_notice!("No driver for class={:06x}", full_class);
			// If a driver can't be found, save the endpoints for later (and the descriptor data)
			Interface::Unknown(endpts, descriptors.to_owned())
			},
		}
	}

	async fn worker(self)
	{
		self.initialise_port(self.addr).await;
		
		let ep0 = ControlEndpoint::new(self.host(), self.addr, /*ep_num=*/0, /*max_packet_size=*/64);
		// Enumerate device
		let interfaces = match self.enumerate(ep0).await
			{
			Ok(v) => v,
			Err(e) => panic!("{}", e),
			};

		log_debug!("{} interfaces", interfaces.len());
		// Await on a wrapper of the interfaces
		struct FutureVec(Vec<Interface>);
		impl ::core::future::Future for FutureVec
		{
			type Output = ();
			fn poll(mut self: ::core::pin::Pin<&mut Self>, cx: &mut ::core::task::Context<'_>) -> ::core::task::Poll<()> {
				for (i,v) in Iterator::enumerate(self.0.iter_mut()) {
					match v
					{
					Interface::Unknown(..) => {
						log_debug!("interface {} unknown", i);
						},
					Interface::Bound(ref mut inst) =>
						match inst.as_mut().poll(cx)
						{
						::core::task::Poll::Pending => {},
						::core::task::Poll::Ready( () ) => todo!("Handle device future completing"),
						},
					}
				}
				::core::task::Poll::Pending
			}
		}
		FutureVec(interfaces).await;
	}
}

/// Representation of an active device interface
enum Interface
{
	/// No fitting driver (yet) - save the endpoints and descriptor data
	Unknown(Vec<Endpoint>, Vec<u8>),
	/// Started driver
	Bound(::core::pin::Pin<crate::device::Instance>),
}

pub enum Endpoint
{
	Control(ControlEndpoint),
	Interrupt(InterruptEndpoint),
	Bulk(BulkEndpoint),
}

pub struct InterruptEndpoint
{
	inner: crate::host::Handle<dyn crate::host::InterruptEndpoint>,
}
impl InterruptEndpoint
{
	fn new(host: &Host, addr: u8, ep_num: u8, max_packet_size: usize, polling_interval: usize) -> Self {
		Self {
			inner: host.driver.init_interrupt(crate::host::EndpointAddr::new(addr, ep_num), max_packet_size, polling_interval),
			}
	}
}

pub struct ControlEndpoint
{
	inner: crate::host::Handle<dyn crate::host::ControlEndpoint>,
}
impl ControlEndpoint
{
	fn new(host: &Host, addr: u8, ep_num: u8, max_packet_size: usize) -> ControlEndpoint {
		ControlEndpoint {
			inner: host.driver.init_control(crate::host::EndpointAddr::new(addr, ep_num), max_packet_size),
			}
	}
	pub async fn read_descriptor_raw(&self, ty: u16, index: u8, buf: &mut [u8]) -> Result<usize,&'static str>
	{
		//log_trace!("read_descriptor_raw: (ty={:#x}, index={}, buf={}b)", ty, index, buf.len());
		let exp_length = buf.len();
		let hdr = hw_decls::DeviceRequest {
			// TODO: These high bits of `ty` aren't present in the returned structure - what are they again?
			req_type: 0x80 | ((ty >> 8) as u8 & 0x3) << 5 | (ty >> 12) as u8 & 3,
			req_num: 6,	// GET_DESCRIPTOR
			value: (ty << 8) | index as u16,
			index: 0,	// TODO: language ID
			length: exp_length as u16,
			};
		let hdr = hdr.to_bytes();
		let res_len = self.inner.in_only(&hdr, buf).await;

		Ok(res_len)
	}
	pub async fn read_descriptor<T>(&self, index: u8) -> Result<T,&'static str>
	where
		T: hw_decls::Descriptor
	{
		let exp_length = ::core::mem::size_of::<T>();
		//log_trace!("read_descriptor: (index={}): exp_length={}", index, exp_length);
		let mut out_data = [0u8; 256];
		let res_len = self.read_descriptor_raw(T::TYPE, index, &mut out_data[..exp_length]).await?;

		match T::from_bytes(&out_data[..res_len])
		{
		Ok(v) => Ok(v),
		Err(hw_decls::ParseError) => Err("parse"),
		}
	}

	// TODO: Better return type?
	pub async fn read_string(&self, index: u8) -> Result<String,&'static str>
	{
		if index == 0 {
			return Ok(String::new());
		}
		let desc: hw_decls::Descriptor_String = self.read_descriptor(index).await?;
		match ::utf16::Str16::new(&desc.utf16[..desc.length as usize / 2 - 1])
		{
		Some(v) => Ok( format!("{}", v) ),
		None => Err("BadStr"),
		}
	}

	pub async fn send_request(&self,  request_type: u8, request_num: u8, value: u16, index: u16, data: &[u8])
	{
		let hdr = hw_decls::DeviceRequest {
			req_type: request_type,
			req_num: request_num,
			value: value,
			index: index,
			length: data.len() as u16,
			};
		let hdr = hdr.to_bytes();
		let sent_len = self.inner.out_only(&hdr, data).await;
		assert_eq!(sent_len, data.len());
	}
}

pub struct BulkEndpoint
{
	inner: crate::host::Handle<dyn crate::host::BulkEndpoint>,
	dir_is_in: bool,
}
impl BulkEndpoint
{
	fn new(host: &Host, addr: u8, ep_num: u8, dir_is_in: bool, max_packet_size: usize) -> Self {
		Self {
			dir_is_in: dir_is_in,
			inner: host.driver.init_bulk(crate::host::EndpointAddr::new(addr, ep_num), max_packet_size),
			}
	}
}

impl Host
{
	fn add_device<F,A>(&self, make_worker: F)
	where
		F: FnOnce(u8) -> A,
		A: ::core::future::Future<Output=()> + Send + 'static,
	{
		// Allocate address
		match self.addresses.lock().allocate()
		{
		Some(v) => {
			assert!(v != 0);
			// Create async task for the device
			let cb = Box::pin(make_worker(v));
			// Insert into the worker list for this host
			let mut lh = self.device_workers[v as usize].lock();
			assert!( lh.is_none(), "Address already allocated?" );
			*lh = Some(cb);
			},
		None => {},
		}
	}

	async fn get_address_zero<'a>(&'a self) -> AddressZeroHandle<'a>
	{
		AddressZeroHandle {
			host: self,
			}
	}

	async fn root_event_task(&self)
	{
		loop
		{
			let port_idx = self.driver.async_wait_root().await;
			self.handle_root_event(port_idx);
		}
	}

	fn handle_root_event(&self, port_idx: usize)
	{
		log_debug!("handle_root_event: ({})", port_idx);

		if self.driver.get_port_feature(port_idx, host::PortFeature::CConnection)
		{
			self.driver.clear_port_feature(port_idx, host::PortFeature::CConnection);
			if self.driver.get_port_feature(port_idx, host::PortFeature::Connection)
			{
				// TODO: This should be unsafe (relies on self being pinned). It's sound... for now
				let hubref = HubRef::Root(HostRef(self));
				self.root_ports[port_idx].signal_connected(hubref, port_idx as u8);
			}
			else
			{
				// Was disconnected, need to eliminate all downstream devices
				// - Requires knowing what devices are on this port.
				// - And need to signal to the devices that they've been disconnected
				todo!("Handle port disconnection");
				//self.ports[port_idx].signal_disconnected();
			}
		}
		/*
		else if self.driver.get_port_feature(port_idx, host::PortFeature::CReset)
		{
			self.driver.clear_port_feature(port_idx, host::PortFeature::CReset);
			if self.driver.get_port_feature(port_idx, host::PortFeature::Reset)
			{
			}
			else if self.driver.get_port_feature(port_idx, host::PortFeature::Enable)
			{
				// Allocate an ID, allocate a , send the 'set device ID' request
				//todo!("Push new device to enumeration");
			}
			else
			{
				// Reset complete, but not enabled?
				todo!("Handle port completing reset, but not being enabled?");
			}
		}
		else if self.driver.get_port_feature(port_idx, host::PortFeature::CEnable)
		{
			self.driver.clear_port_feature(port_idx, host::PortFeature::CEnable);
			log_debug!("Change in enable status...");
		}
		*/
		else
		{
		}
	}
}

struct AddressZeroHandle<'a> {
	host: &'a Host,
}
impl<'a> AddressZeroHandle<'a>
{
	async fn send_setup_address(&self, addr: u8) {
		// Send a request with type=0x00, request=5,  value=addr, index=0, and no data
		self.host.endpoint_zero_handle.send_request(0x00, 5, addr as u16, 0, &[]).await
	}
}
impl<'a> ::core::ops::Drop for AddressZeroHandle<'a>
{
	fn drop(&mut self)
	{
	}
}

impl AddressPool
{
	fn allocate(&mut self) -> Option<u8>
	{
		for i in self.next_id ..= 255 {
			let byte = &mut self.used_ids[i as usize / 8];
			let bitmask = 1 << (i%8);
			if 0 == *byte & bitmask {
				*byte |= bitmask;
				self.next_id = i.checked_add(1).unwrap_or(1);
				return Some(i);
			}
		}
		// Wraparound!
		for i in 1 .. self.next_id {
			let byte = &mut self.used_ids[i as usize / 8];
			let bitmask = 1 << (i%8);
			if 0 == *byte & bitmask {
				*byte |= bitmask;
				self.next_id = i.checked_add(1).unwrap_or(1);
				return Some(i);
			}
		}
		// Exhausted
		None
	}
}

impl HubDevice
{
	fn handle_int(&self, _size: usize)
	{
		let data_handle = self.int_ep.get_data();
		let data = data_handle.get();
		todo!("Process interrupt bytes from host - {:?}", ::kernel::logging::HexDump(data));
	}

	async fn set_port_feature(&self, port_idx: usize, feat: host::PortFeature) {
		todo!("HubDevice::set_port_feature({}, {:?})", port_idx, feat)
	}
	async fn clear_port_feature(&self, port_idx: usize, feat: host::PortFeature) {
		todo!("HubDevice::clear_port_feature({}, {:?})", port_idx, feat)
	}
	async fn get_port_feature(&self, port_idx: usize, feat: host::PortFeature) -> bool {
		todo!("HubDevice::get_port_feature({}, {:?})", port_idx, feat)
	}

}


