//! An SMB-aware TCP proxy that caps the credit window a server grants, for
//! testing what the backend does against a server whose window is too small
//! to fund one big compound READ.
//!
//! Neither reference fixture can play that server: Samba grants every credit
//! smb2 asks for (smb2 steers toward 512), and no fixture limits it. So the
//! proxy sits between the client and the ordinary guest fixture and rewrites
//! the `CreditResponse` field of every response header on the way back, so the
//! client never holds more than `cap` credits. The server never learns: it
//! thinks it granted plenty, and the client simply uses fewer message ids.
//!
//! It tracks each connection's balance the way the client does (grants in,
//! charges out, one credit to start), and only ever lowers a grant. A grant
//! that would leave the balance empty is lowered only as far as one credit, so
//! the connection can't deadlock.
//!
//! ❗ Rewriting headers is only possible on unsigned, unencrypted traffic,
//! which is what the guest fixture speaks. A signed, encrypted, or compressed
//! frame fails the test loudly rather than passing through unclamped.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const SMB2_MAGIC: [u8; 4] = [0xFE, b'S', b'M', b'B'];
const HEADER_LEN: usize = 64;
const CREDIT_CHARGE_AT: usize = 6;
const CREDITS_AT: usize = 14;
const FLAGS_AT: usize = 16;
const NEXT_COMMAND_AT: usize = 20;
const FLAG_SIGNED: u32 = 0x0000_0008;

/// A proxy from a loopback port of its own to one SMB server, capping every
/// connection's credit window at `cap`. Dropping it stops the listener.
pub(super) struct CreditCapProxy {
    port: u16,
    listener: JoinHandle<()>,
}

impl CreditCapProxy {
    /// Starts forwarding from a fresh `127.0.0.1` port to `127.0.0.1:target_port`.
    pub(super) async fn start(target_port: u16, cap: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("binding the credit-cap proxy to a loopback port");
        let port = listener.local_addr().expect("the proxy's own address").port();
        let listener = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(forward(client, target_port, cap));
            }
        });
        Self { port, listener }
    }

    /// The loopback port to point the client at.
    pub(super) fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for CreditCapProxy {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

/// The client's credit balance on one connection, as the proxy sees it.
struct Balance {
    held: i64,
    cap: i64,
}

async fn forward(client: TcpStream, target_port: u16, cap: u16) {
    let Ok(server) = TcpStream::connect(("127.0.0.1", target_port)).await else {
        return;
    };
    let balance = Arc::new(Mutex::new(Balance {
        held: 1,
        cap: i64::from(cap),
    }));
    let (client_read, client_write) = client.into_split();
    let (server_read, server_write) = server.into_split();
    tokio::join!(
        pump(client_read, server_write, Arc::clone(&balance), Direction::Requests),
        pump(server_read, client_write, balance, Direction::Responses),
    );
}

#[derive(Clone, Copy)]
enum Direction {
    Requests,
    Responses,
}

/// Copies one direction frame by frame (a 4-byte NetBIOS length, then the
/// SMB2 message chain), charging requests and clamping responses.
async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, balance: Arc<Mutex<Balance>>, direction: Direction) {
    loop {
        let mut prefix = [0u8; 4];
        if from.read_exact(&mut prefix).await.is_err() {
            let _ = to.shutdown().await;
            return;
        }
        let len = u32::from_be_bytes([0, prefix[1], prefix[2], prefix[3]]) as usize;
        let mut frame = vec![0u8; len];
        if from.read_exact(&mut frame).await.is_err() {
            let _ = to.shutdown().await;
            return;
        }
        rewrite_chain(&mut frame, &balance, direction);
        if to.write_all(&prefix).await.is_err() || to.write_all(&frame).await.is_err() {
            return;
        }
    }
}

/// Walks every header in one compound chain.
fn rewrite_chain(frame: &mut [u8], balance: &Mutex<Balance>, direction: Direction) {
    assert!(
        frame.len() >= HEADER_LEN && frame[..4] == SMB2_MAGIC,
        "the credit-cap proxy only understands plain SMB2 frames; got {:02x?}",
        &frame[..frame.len().min(4)]
    );
    let mut at = 0;
    loop {
        let header = &mut frame[at..at + HEADER_LEN];
        let flags = u32::from_le_bytes(header[FLAGS_AT..FLAGS_AT + 4].try_into().expect("four bytes"));
        assert!(
            flags & FLAG_SIGNED == 0,
            "a signed frame can't have its credits rewritten"
        );
        let mut state = balance
            .lock()
            .expect("the balance lock poisoned: another proxy direction panicked mid-frame, so the test is already failing");
        match direction {
            Direction::Requests => {
                let charge = u16::from_le_bytes([header[CREDIT_CHARGE_AT], header[CREDIT_CHARGE_AT + 1]]).max(1);
                state.held -= i64::from(charge);
            }
            Direction::Responses => {
                let granted = i64::from(u16::from_le_bytes([header[CREDITS_AT], header[CREDITS_AT + 1]]));
                let room = (state.cap - state.held).max(0);
                let mut allowed = granted.min(room);
                if state.held + allowed < 1 {
                    // Never strand the connection with nothing to send on, and
                    // never grant past the server: a message id it didn't fund
                    // gets the connection dropped.
                    allowed = granted.min(1 - state.held);
                }
                state.held += allowed;
                let allowed = u16::try_from(allowed).expect("a clamped grant fits a u16");
                header[CREDITS_AT..CREDITS_AT + 2].copy_from_slice(&allowed.to_le_bytes());
            }
        }
        let next = u32::from_le_bytes(
            header[NEXT_COMMAND_AT..NEXT_COMMAND_AT + 4]
                .try_into()
                .expect("four bytes"),
        );
        if next == 0 {
            return;
        }
        at += next as usize;
    }
}
