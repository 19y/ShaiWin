//! Off-thread work: snapshots, icons and the "get ready to park" plan.
//!
//! `PrintWindow` talks to the target application and can block for as long as
//! that application is unresponsive, so none of it may run on the UI thread.
//! One worker is enough: hangs are human-paced.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Instant;

use shaiwin_platform_win::park::{ParkError, ParkPlan};
use shaiwin_platform_win::window::WindowInfo;
use shaiwin_platform_win::{capture, icon, park, Image};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app::HangSource;
use crate::WM_EVENT;

pub enum Job {
    /// Take the picture, then work out how to park the window. Mutates nothing.
    ///
    /// `asked` is when the UI thread decided to hang it: one worker serves every
    /// job, so without the stamp a hang that waited behind an icon or a
    /// recovery snapshot looks exactly like a slow snapshot.
    Prepare {
        info: WindowInfo,
        source: HangSource,
        asked: Instant,
    },
    /// Re-take a picture of a window that is already parked (startup recovery).
    Snapshot { id: u64, hwnd: isize },
    /// Pull an application icon out of its executable.
    Icon { id: u64, exe_path: String },
}

pub enum Reply {
    Prepared {
        info: WindowInfo,
        image: Option<Image>,
        plan: Result<ParkPlan, ParkError>,
        source: HangSource,
    },
    Snapshotted {
        id: u64,
        image: Option<Image>,
    },
    Iconed {
        id: u64,
        image: Option<Image>,
    },
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
                        Job::Prepare {
                            info,
                            source,
                            asked,
                        } => {
                            let started = Instant::now();
                            let shot = capture::snapshot(info.hwnd);
                            let shot_ms = started.elapsed().as_millis();
                            let started = Instant::now();
                            let plan = park::plan(shaiwin_platform_win::window::hwnd_of(info.hwnd));
                            let plan_ms = started.elapsed().as_millis();
                            // The whole cost of getting ready, in the order it is
                            // paid: waiting for the worker, the picture, the plan.
                            match shot.image.as_ref() {
                                Some(image) => crate::log!(
                                    "shot {:#x}: {} {}x{} (waited {} ms, shot {} ms of which settled {}, plan {} ms)",
                                    info.hwnd,
                                    shot.via,
                                    image.width,
                                    image.height,
                                    asked.elapsed().as_millis(),
                                    shot_ms,
                                    shot.settled_ms,
                                    plan_ms
                                ),
                                None => crate::log!(
                                    "shot {:#x}: no picture ({}, waited {} ms, shot {} ms of which settled {}, plan {} ms)",
                                    info.hwnd,
                                    shot.via,
                                    asked.elapsed().as_millis(),
                                    shot_ms,
                                    shot.settled_ms,
                                    plan_ms
                                ),
                            }
                            Reply::Prepared {
                                info,
                                image: shot.image,
                                plan,
                                source,
                            }
                        }
                        Job::Snapshot { id, hwnd } => {
                            let shot = capture::snapshot(hwnd);
                            crate::log!(
                                "shot {:#x}: recovery {} (settled {} ms)",
                                hwnd,
                                shot.via,
                                shot.settled_ms
                            );
                            Reply::Snapshotted {
                                id,
                                image: shot.image,
                            }
                        }
                        Job::Icon { id, exe_path } => Reply::Iconed {
                            id,
                            image: icon::for_exe(&exe_path, 48),
                        },
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
        Worker {
            jobs: job_tx,
            replies: reply_rx,
        }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}
