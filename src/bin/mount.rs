#![deny(warnings)]
#![cfg_attr(unix, feature(libc))]

#[cfg(unix)]
extern crate libc;

#[cfg(target_os = "redox")]
extern crate syscall;

extern crate redoxfs;
extern crate uuid;

use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::process;

use redoxfs::{DiskCache, DiskFile, mount};
use uuid::Uuid;

#[cfg(unix)]
fn fork() -> isize {
    unsafe { libc::fork() as isize }
}

#[cfg(unix)]
fn pipe(pipes: &mut [i32; 2]) -> isize {
    unsafe { libc::pipe(pipes.as_mut_ptr()) as isize }
}

#[cfg(target_os = "redox")]
fn fork() -> isize {
    unsafe { syscall::Error::mux(syscall::clone(0)) as isize }
}

#[cfg(target_os = "redox")]
fn pipe(pipes: &mut [usize; 2]) -> isize {
    syscall::Error::mux(syscall::pipe2(pipes, 0)) as isize
}

fn usage() {
    println!("redoxfs [--uuid] [disk or uuid] [mountpoint]");
}

enum DiskId {
    Path(String),
    Uuid(Uuid),
}

#[cfg(not(target_os = "redox"))]
fn disk_paths(_paths: &mut Vec<String>) {}

#[cfg(target_os = "redox")]
fn disk_paths(paths: &mut Vec<String>) {
    use std::fs;

    let mut schemes = vec![];
    match fs::read_dir(":") {
        Ok(entries) => for entry_res in entries {
            if let Ok(entry) = entry_res {
                if let Ok(path) = entry.path().into_os_string().into_string() {
                    let scheme = path.trim_left_matches(':').trim_matches('/');
                    if scheme.starts_with("disk") {
                        println!("redoxfs: found scheme {}", scheme);
                        schemes.push(format!("{}:", scheme));
                    }
                }
            }
        },
        Err(err) => {
            println!("redoxfs: failed to list schemes: {}", err);
        }
    }

    for scheme in schemes {
        match fs::read_dir(&scheme) {
            Ok(entries) => for entry_res in entries {
                if let Ok(entry) = entry_res {
                    if let Ok(path) = entry.path().into_os_string().into_string() {
                        println!("redoxfs: found path {}", path);
                        paths.push(path);
                    }
                }
            },
            Err(err) => {
                println!("redoxfs: failed to list '{}': {}", scheme, err);
            }
        }
    }
}

fn daemon(disk_id: &DiskId, mountpoint: &str, mut write: File) -> ! {
    let mut paths = vec![];
    let mut uuid_opt = None;

    match *disk_id {
        DiskId::Path(ref path) => {
            paths.push(path.clone());
        },
        DiskId::Uuid(ref uuid) => {
            disk_paths(&mut paths);
            uuid_opt = Some(uuid.clone());
        },
    }

    for path in paths {
        println!("redoxfs: opening {}", path);
        match DiskFile::open(&path).map(|image| DiskCache::new(image)) {
            Ok(disk) => match redoxfs::FileSystem::open(disk) {
                Ok(filesystem) => {
                    println!("redoxfs: opened filesystem on {} with uuid {}", path,
                             Uuid::from_bytes(&filesystem.header.1.uuid).unwrap().hyphenated());

                    let matches = if let Some(uuid) = uuid_opt {
                        if &filesystem.header.1.uuid == uuid.as_bytes() {
                            println!("redoxfs: filesystem on {} matches uuid {}", path, uuid.hyphenated());
                            true
                        } else {
                            println!("redoxfs: filesystem on {} does not match uuid {}", path, uuid.hyphenated());
                            false
                        }
                    } else {
                        true
                    };

                    if matches {
                        match mount(filesystem, &mountpoint, || {
                            println!("redoxfs: mounted filesystem on {} to {}", path, mountpoint);
                            let _ = write.write(&[0]);
                        }) {
                            Ok(()) => {
                                process::exit(0);
                            },
                            Err(err) => {
                                println!("redoxfs: failed to mount {} to {}: {}", path, mountpoint, err);
                            }
                        }
                    }
                },
                Err(err) => println!("redoxfs: failed to open filesystem {}: {}", path, err)
            },
            Err(err) => println!("redoxfs: failed to open image {}: {}", path, err)
        }
    }

    match *disk_id {
        DiskId::Path(ref path) => {
            println!("redoxfs: not able to mount path {}", path);
        },
        DiskId::Uuid(ref uuid) => {
            println!("redoxfs: not able to mount uuid {}", uuid.hyphenated());
        },
    }

    let _ = write.write(&[1]);
    process::exit(1);
}

fn main() {
    let mut args = env::args().skip(1);

    let disk_id = match args.next() {
        Some(arg) => if arg == "--uuid" {
            let uuid = match args.next() {
                Some(arg) => match Uuid::parse_str(&arg) {
                    Ok(uuid) => uuid,
                    Err(err) => {
                        println!("redoxfs: invalid uuid '{}': {}", arg, err);
                        usage();
                        process::exit(1);
                    }
                },
                None => {
                    println!("redoxfs: no uuid provided");
                    usage();
                    process::exit(1);
                }
            };

            DiskId::Uuid(uuid)
        } else {
            DiskId::Path(arg)
        },
        None => {
            println!("redoxfs: no disk provided");
            usage();
            process::exit(1);
        }
    };

    let mountpoint = match args.next() {
        Some(arg) => arg,
        None => {
            println!("redoxfs: no mountpoint provided");
            usage();
            process::exit(1);
        }
    };

    let mut pipes = [0; 2];
    if pipe(&mut pipes) == 0 {
        let mut read = unsafe { File::from_raw_fd(pipes[0]) };
        let write = unsafe { File::from_raw_fd(pipes[1]) };

        let pid = fork();
        if pid == 0 {
            drop(read);

            daemon(&disk_id, &mountpoint, write);
        } else if pid > 0 {
            drop(write);

            let mut res = [0];
            read.read(&mut res).unwrap();

            process::exit(res[0] as i32);
        } else {
            panic!("redoxfs: failed to fork");
        }
    } else {
        panic!("redoxfs: failed to create pipe");
    }
}
