//! Off-thread work: snapshots, icons and the "get ready to park" plan.
//!
//! `PrintWindow` talks to the target application and can block for as long as
//! that application is unresponsive, so none of it may run on the UI thread.
//! One worker is enough: hangs are human-paced.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use shaiwin_platform_win::park::{ParkError, ParkPlan};
use shaiwin_platform_win::window::WindowInfo;
use shaiwin_platform_win::{Image, capture, icon, park};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::WM_EVENT;

pub enum Job {
    /// Take the picture, then work out how to park the window. Mutates nothing.
    Prepare { info: WindowInfo },
    /// Re-take a picture of a window that is already parked (startup recovery).
    Snapshot { id: u64, hwnd: isize },
    /// Pull an application icon out of its executable.
    Icon { id: u64, exe_path: String },
}

pub enum Reply {
    Prepared { info: WindowInfo, image: Option<Image>, plan: Result<ParkPlan, ParkError> },
    Snapshotted { id: u64, image: Option<Image> },
    Iconed { id: u64, image: Option<Image> },
}

pub struct Worker {
    jobs: Sender<Job>,
    pub replies: Receiver<Reply>,
}

impl Worker {
    pub fn spawn(hwnd: isize) -> Worker {
        let (job_tx, job_rx) = channel::<Job>();
        let (reply_tx, reply_rx) = channel::<Reply>();
        thread::Builder::new()
            .name("shaiwin-worker".into())
            .spawn(move || {
                let target = shaiwin_platform_win::window::hwnd_of(hwnd);
                while let Ok(job) = job_rx.recv() {
                    let reply = match job {
                        Job::Prepare { info } => {
                            let shot = capture::snapshot(info.hwnd);
                            match shot.image.as_ref() {
                                Some(image) => crate::log!(
                                    "shot {:#x}: {} {}x{} (settled {} ms)",
                                    info.hwnd,
                                    shot.via,
                                    image.width,
                                    image.height,
                                    shot.settled_ms
                                ),
                                None => crate::log!(
                                    "shot {:#x}: no picture ({}, settled {} ms)",
                                    info.hwnd,
                                    shot.via,
                                    shot.settled_ms
                                ),
                            }
                            let plan = park::plan(shaiwin_platform_win::window::hwnd_of(info.hwnd));
                            Reply::Prepared { info, image: shot.image, plan }
                        }
                        Job::Snapshot { id, hwnd } => {
                            let shot = capture::snapshot(hwnd);
                            crate::log!("shot {:#x}: recovery {} (settled {} ms)", hwnd, shot.via, shot.settled_ms);
                            Reply::Snapshotted { id, image: shot.image }
                        }
                        Job::Icon { id, exe_path } => Reply::Iconed { id, image: icon::for_exe(&exe_path, 48) },
                    };
                    if reply_tx.send(reply).is_err() {
                        break;
                    }
                    unsafe {
                        let _ = PostMessageW(Some(target), WM_EVENT, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .expect("worker thread");
        Worker { jobs: job_tx, replies: reply_rx }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}
