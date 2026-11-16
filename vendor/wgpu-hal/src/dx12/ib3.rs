//! Temporary attribution build. Thread-local counters, no per-draw I/O. Drain at frame 34.
use std::{cell::RefCell, time::Instant, sync::atomic::{AtomicUsize,Ordering::Relaxed}};
use alloc::vec::Vec;
pub const CREATE:usize=0; pub const RESET:usize=1; pub const CLOSE:usize=2; pub const A_RESET:usize=3;
pub const BARRIER:usize=4; pub const SURFACE_BARRIER:usize=5; pub const COPY_TEX:usize=6; pub const COPY_BUF:usize=7;
pub const EXECUTE:usize=8; pub const BIND:usize=9; pub const RTV:usize=10;pub const PASS:usize=11;
pub const PREP:usize=12; pub const FINISH:usize=13; pub const SUBMIT:usize=14; pub const PRESENT:usize=15;
const K:usize=16;
#[derive(Default,Debug)]
struct Row { index:usize, buffers:usize, width:u32, height:u32, kinds:[usize;16],layers:usize, targets:usize, calls:[u32;K], ns:[u64;K], live:usize, peak:usize }
#[derive(Default)]
struct Meter { current:Row, back:usize, rows:Vec<Row>, ended:bool }
std::thread_local! { static S:RefCell<Meter>=RefCell::default(); }
pub static LIVE:AtomicUsize=AtomicUsize::new(0);
pub static PEAK:AtomicUsize=AtomicUsize::new(0);
pub static LISTS:AtomicUsize=AtomicUsize::new(0);
pub fn shared()->bool{static ON:std::sync::OnceLock<bool>=std::sync::OnceLock::new();*ON.get_or_init(||std::env::var_os("IB3_SHARED").is_some())}
std::thread_local!{static IDS:RefCell<([usize;64],[usize;64],usize,u32)>=RefCell::new(([0;64],[0;64],0,0));}
pub fn opened(a:usize,l:usize){IDS.with_borrow_mut(|s|{if s.3<12&&s.2<64{let n=s.2;s.0[n]=a;s.1[n]=l;s.2+=1;}})}
pub fn ids(){IDS.with_borrow_mut(|s|{if s.3<12{std::eprintln!("IBIDS {} {:?} {:?}",s.3,&s.0[..s.2],&s.1[..s.2]);s.2=0;}s.3+=1;})}
pub fn add(n:usize,t:Instant) {if n==CREATE{LISTS.fetch_add(1,Relaxed);}let ns=t.elapsed().as_nanos() as u64; S.with_borrow_mut(|s|{s.current.calls[n]+=1;s.current.ns[n]+=ns;});}
pub fn surface(index:usize,buffers:usize,back:usize,w:u32,h:u32) { S.with_borrow_mut(|s| {s.back=back; s.current.index=index; s.current.buffers=buffers; s.current.width=w; s.current.height=h;}); }
pub fn back(ptr:usize)->bool{S.with_borrow(|s|s.back==ptr)}
pub fn scene(kinds:[usize;16],layers:usize,targets:usize){S.with_borrow_mut(|s|{s.current.kinds=kinds;s.current.layers=layers;s.current.targets=targets;});}
pub fn end(){S.with_borrow_mut(|s|{if s.ended{return} let mut r=core::mem::take(&mut s.current);r.live=LIVE.load(Relaxed);r.peak=PEAK.load(Relaxed);s.rows.push(r);if s.rows.len()==34{ s.ended=true;std::eprintln!("IBLISTS {}",LISTS.load(Relaxed));for (i,r) in s.rows.iter().enumerate(){std::eprintln!("IBD {i} {} {} {} {} {} {} {} {} {:?} {:?} {:?}",r.index,r.buffers,r.width,r.height,r.layers,r.targets,r.live,r.peak,r.kinds,r.calls,r.ns);} }});}
