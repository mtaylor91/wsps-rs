use std::os::unix::io::{AsRawFd, RawFd};
use std::io;
use std::net::TcpListener;
use libc;


macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}


const READ_FLAGS: i32 = libc::EPOLLONESHOT | libc::EPOLLIN;
const WRITE_FLAGS: i32 = libc::EPOLLONESHOT | libc::EPOLLOUT;


fn epoll_create() -> io::Result<RawFd> {
    let fd = syscall!(epoll_create1(libc::FD_CLOEXEC))?;
    Ok(fd)
}


fn epoll_event_read(key: u64) -> libc::epoll_event {
    libc::epoll_event {
        events: READ_FLAGS as u32,
        u64: key,
    }
}


fn main() -> io::Result<()> {
    // Create TCP listener
    let listener = TcpListener::bind("127.0.0.1:9001")?;
    listener.set_nonblocking(true)?;

    // Get the file descriptor for the listener
    let listener_fd = listener.as_raw_fd();

    // Create epoll instance
    let epoll_fd = epoll_create()?;

    // Register interest in listener incoming connections
    let mut key = 100;  // key used to identify the fd associated with the event
    syscall!(epoll_ctl(
        epoll_fd,
        libc::EPOLL_CTL_ADD,
        listener_fd,
        &mut epoll_event_read(key),
    ))?;
    
    // Create vector to store incoming epoll events
    let mut events: Vec<libc::epoll_event> = Vec::with_capacity(1024);

    loop {
        // Clear the events vector
        events.clear();

        // Wait for epoll events
        let result_count = match syscall!(epoll_wait(
                epoll_fd,
                events.as_mut_ptr() as *mut libc::epoll_event,
                events.capacity() as i32,
                -1,
        )) {
            Ok(count) => count,
            Err(err) => {
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                } else {
                    return Err(err);
                }
            }
        };

        // Safety: epoll_wait guarantees that the events slice is valid
        unsafe { events.set_len(result_count as usize); }

        // Process epoll events
        for evt in &events {
            match evt.u64 {
                100 => {
                    match listener.accept() {
                        Ok((stream, addr)) => {
                            stream.set_nonblocking(true)?;

                            println!("Accepted connection from {}", addr);

                            // Increment key
                            key += 1;

                            // Register interest in read events
                            syscall!(epoll_ctl(
                                epoll_fd,
                                libc::EPOLL_CTL_ADD,
                                stream.as_raw_fd(),
                                &mut epoll_event_read(key),
                            ))?;
                        },
                        Err(err) => {
                            println!("Error accepting connection: {}", err);
                        }
                    }

                    // Re-register interest in listener incoming connections
                    syscall!(epoll_ctl(
                        epoll_fd,
                        libc::EPOLL_CTL_MOD,
                        listener_fd,
                        &mut epoll_event_read(key),
                    ))?;
                },
                // TODO: Handle other events
                _ => (),
            }
        }
    }
}
