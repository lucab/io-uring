use anyhow; // anyhow = "1.0"
use io_uring; // io-uring = "0.5"

fn main() -> anyhow::Result<()> {
    let mut ring = io_uring::IoUring::new(32)?;

    let socket_slot = {
        use std::os::fd::AsRawFd;
        let server_sock = std::net::UdpSocket::bind("127.0.0.1:1234")?;
        ring.submitter()
            .register_files(&[server_sock.as_raw_fd()])?;
        io_uring::types::Fixed(0)
    };

    // Provide 2 buffers in buffer group `33`, at index 0 and 1.
    // Each one is 512 bytes large.
    const BUF_GROUP: u16 = 33;
    const SIZE: usize = 512;
    let mut buffers = [[0u8; SIZE]; 2];
    for (index, buf) in buffers.iter_mut().enumerate() {
        let provide_bufs_e = io_uring::opcode::ProvideBuffers::new(
            buf.as_mut_ptr(),
            SIZE as i32,
            1,
            BUF_GROUP,
            index as u16,
        )
        .build()
        .user_data(11);
        unsafe { ring.submission().push(&provide_bufs_e)? };
        ring.submitter().submit_and_wait(1)?;
        let cqes: Vec<io_uring::cqueue::Entry> = ring.completion().map(Into::into).collect();
        assert_eq!(cqes.len(), 1);
        assert_eq!(cqes[0].user_data(), 11);
        assert_eq!(cqes[0].result(), 0);
        assert_eq!(cqes[0].flags(), 0);
    }

    // This is used as input.
    let mut msghdr: libc::msghdr = unsafe { std::mem::zeroed() };
    msghdr.msg_namelen = 32;
    msghdr.msg_controllen = 0;

    const IORING_RECV_MULTISHOT: u16 = 2;
    let recvmsg_e = io_uring::opcode::RecvMsg::new(socket_slot, &mut msghdr as *mut _)
        .ioprio(IORING_RECV_MULTISHOT)
        .buf_group(BUF_GROUP)
        .build()
        .flags(io_uring::squeue::Flags::BUFFER_SELECT)
        .user_data(77);
    unsafe { ring.submission().push(&recvmsg_e)? };
    ring.submitter().submit_and_wait(1)?;

    loop {
        ring.completion().sync();
        for c_entry in ring.completion() {
            eprintln!("{:?}", c_entry);
            if c_entry.user_data() != 77 {
                continue;
            }
            if c_entry.result() >= 0 {
                let buf_id = io_uring::cqueue::buffer_select(c_entry.flags()).unwrap() as usize;
                let out = io_uring::types::RecvMsgOut::parse(
                    &buffers[buf_id],
                    msghdr.msg_namelen,
                    msghdr.msg_controllen as u32,
                )
                .unwrap();
                eprintln!("RECVMSG: {:?}", out);
                if !io_uring::cqueue::more(c_entry.flags()) {
                    anyhow::bail!("Multishot ended");
                }
            } else {
                anyhow::bail!("recvmsg failed ({})", c_entry.result());
            }
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}
