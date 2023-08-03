use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::{AsRawFd, RawFd};
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


const READ_BUFFER_SIZE: usize = 1024;
const WRITE_BUFFER_SIZE: usize = 1024;
const READ_FLAGS: i32 = libc::EPOLLONESHOT | libc::EPOLLIN;
const WRITE_FLAGS: i32 = libc::EPOLLONESHOT | libc::EPOLLOUT;


struct Connection {
    key: u64,
    stream: TcpStream,
    read_buf: [u8; READ_BUFFER_SIZE],
    read_len: usize,
    write_buf: [u8; WRITE_BUFFER_SIZE],
    write_len: usize,
}

impl Connection {
    fn new(key: u64, stream: TcpStream) -> Self {
        Self {
            key,
            stream,
            read_buf: [0; READ_BUFFER_SIZE],
            read_len: 0,
            write_buf: [0; WRITE_BUFFER_SIZE],
            write_len: 0,
        }
    }

    fn epoll_flags(&self) -> i32 {
        let mut flags = 0;

        if self.read_len < self.read_buf.len() {
            flags |= READ_FLAGS;
        }

        if self.write_len > 0 {
            flags |= WRITE_FLAGS;
        }

        flags
    }

    fn read(&mut self) -> io::Result<()> {
        // Read data from the connection
        match self.stream.read(&mut self.read_buf[self.read_len..]) {
            Ok(0) => {
                // EOF
                println!("Connection closed by peer");
                Err(io::ErrorKind::UnexpectedEof.into())
            },
            Ok(n) => {
                println!("Read {} bytes", n);
                Ok(())
            },
            Err(err) => {
                Err(err)
            }
        }
    }

    fn write(&mut self) -> io::Result<()> {
        // Write data to the connection
        match self.stream.write(&self.write_buf[..self.write_len]) {
            Ok(0) => {
                // EOF
                println!("Connection closed by peer");
                Err(io::ErrorKind::UnexpectedEof.into())
            },
            Ok(n) => {
                self.write_len -= n;
                println!("Wrote {} bytes", n);
                Ok(())
            },
            Err(err) => {
                Err(err)
            }
        }
    }
}


fn epoll_create() -> io::Result<RawFd> {
    let fd = syscall!(epoll_create1(0))?;
    Ok(fd)
}


fn epoll_ctl_add(epoll_fd: RawFd, fd: RawFd, key: u64, flags: i32) -> io::Result<()> {
    let mut evt = libc::epoll_event{events: flags as u32, u64: key};
    syscall!(epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut evt))?;
    Ok(())
}


fn epoll_ctl_mod(epoll_fd: RawFd, fd: RawFd, key: u64, flags: i32) -> io::Result<()> {
    let mut evt = libc::epoll_event{events: flags as u32, u64: key};
    syscall!(epoll_ctl(epoll_fd, libc::EPOLL_CTL_MOD, fd, &mut evt))?;
    Ok(())
}


fn main() -> io::Result<()> {
    // Create TCP listener
    println!("Starting TCP listener on 127.0.0.1:9001");
    let listener = TcpListener::bind("127.0.0.1:9001")?;
    listener.set_nonblocking(true)?;

    // Get the file descriptor for the listener
    let listener_fd = listener.as_raw_fd();

    // Create epoll instance
    println!("Creating epoll instance");
    let epoll_fd = epoll_create()?;

    // Register interest in listener incoming connections
    println!("Registering interest in incoming connections");
    let mut key = 100;  // key used to identify the fd associated with the event
    epoll_ctl_add(epoll_fd, listener_fd, key, READ_FLAGS)?;
    
    // Create vector to store incoming epoll events
    let mut events: Vec<libc::epoll_event> = Vec::with_capacity(1024);

    let mut connections: HashMap<u64, Connection> = HashMap::new();

    println!("Entering event loop");
    loop {
        // Clear the events vector
        events.clear();

        // Wait for epoll events
        println!("Waiting for epoll events");
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
            let evt_key = evt.u64;
            println!("Event: {}", evt_key);
            match evt.u64 {
                100 => {
                    match listener.accept() {
                        Ok((stream, addr)) => {
                            stream.set_nonblocking(true)?;

                            println!("Accepted connection from {}", addr);

                            // Increment key
                            key += 1;

                            // Register interest in read events
                            let stream_fd = stream.as_raw_fd();
                            epoll_ctl_add(epoll_fd, stream_fd, key, READ_FLAGS)?;

                            // Store connection
                            connections.insert(key, Connection::new(key, stream));
                        },
                        Err(err) => {
                            println!("Error accepting connection: {}", err);
                        }
                    }

                    // Re-register interest in listener incoming connections
                    epoll_ctl_mod(epoll_fd, listener_fd, 100, READ_FLAGS)?;
                },
                // Handle other events
                key => {
                    if let Some(conn) = connections.get_mut(&key) {
                        if evt.events & libc::EPOLLIN as u32 != 0 {
                            // Read data from the connection
                            match conn.read() {
                                Ok(()) => {
                                    // Re-register interest in read events
                                    epoll_ctl_mod(
                                        epoll_fd,
                                        conn.stream.as_raw_fd(),
                                        conn.key,
                                        conn.epoll_flags(),
                                    )?;
                                },
                                Err(err) => {
                                    if err.kind() == io::ErrorKind::WouldBlock {
                                        // Re-register interest in read events
                                        epoll_ctl_mod(
                                            epoll_fd,
                                            conn.stream.as_raw_fd(),
                                            conn.key,
                                            conn.epoll_flags(),
                                        )?;
                                    } else {
                                        let conn_key = conn.key;
                                        connections.remove(&conn_key);
                                        println!(
                                            "Error reading from connection: {}",
                                            err,
                                        );
                                    }
                                }
                            }
                        } else if evt.events & libc::EPOLLOUT as u32 != 0 {
                            // Write data to the connection
                            match conn.write() {
                                Ok(()) => {
                                    // Re-register interest in write events
                                    epoll_ctl_mod(
                                        epoll_fd,
                                        conn.stream.as_raw_fd(),
                                        conn.key,
                                        conn.epoll_flags(),
                                    )?;
                                },
                                Err(err) => {
                                    if err.kind() == io::ErrorKind::WouldBlock {
                                        // Re-register interest in write events
                                        epoll_ctl_mod(
                                            epoll_fd,
                                            conn.stream.as_raw_fd(),
                                            conn.key,
                                            conn.epoll_flags(),
                                        )?;
                                    } else {
                                        let conn_key = conn.key;
                                        connections.remove(&conn_key);
                                        println!(
                                            "Error writing to connection: {}",
                                            err,
                                        );
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}
