#![no_std]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rights {
    Send,
    Receive,
}
#[derive(Clone, Copy)]
struct Capability {
    endpoint: usize,
    rights: Rights,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub sender: usize,
    pub word: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidProcess,
    InvalidHandle,
    InvalidEndpoint,
    Denied,
    Full,
    Empty,
    Occupied,
}
struct Queue<const N: usize> {
    messages: [Message; N],
    head: usize,
    len: usize,
}
impl<const N: usize> Queue<N> {
    const fn empty() -> Self {
        Self {
            messages: [Message { sender: 0, word: 0 }; N],
            head: 0,
            len: 0,
        }
    }
    fn send(&mut self, message: Message) -> Result<(), Error> {
        if self.len == N {
            return Err(Error::Full);
        }
        self.messages[(self.head + self.len) % N] = message;
        self.len += 1;
        Ok(())
    }
    fn receive(&mut self) -> Result<Message, Error> {
        if self.len == 0 {
            return Err(Error::Empty);
        }
        let message = self.messages[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Ok(message)
    }
}

/// Handles are local to a process; only trusted kernel code can install grants.
pub struct Broker<const P: usize, const C: usize, const E: usize, const Q: usize> {
    capabilities: [[Option<Capability>; C]; P],
    queues: [Queue<Q>; E],
}
impl<const P: usize, const C: usize, const E: usize, const Q: usize> Broker<P, C, E, Q> {
    pub const fn empty() -> Self {
        assert!(Q > 0);
        Self {
            capabilities: [[None; C]; P],
            queues: [const { Queue::empty() }; E],
        }
    }
    pub fn grant(
        &mut self,
        pid: usize,
        handle: usize,
        endpoint: usize,
        rights: Rights,
    ) -> Result<(), Error> {
        if endpoint >= E {
            return Err(Error::InvalidEndpoint);
        }
        let slot = self
            .capabilities
            .get_mut(pid)
            .ok_or(Error::InvalidProcess)?
            .get_mut(handle)
            .ok_or(Error::InvalidHandle)?;
        if slot.is_some() {
            return Err(Error::Occupied);
        }
        *slot = Some(Capability { endpoint, rights });
        Ok(())
    }
    fn endpoint(&self, pid: usize, handle: u64, rights: Rights) -> Result<usize, Error> {
        let handle = usize::try_from(handle).map_err(|_| Error::InvalidHandle)?;
        let cap = self
            .capabilities
            .get(pid)
            .ok_or(Error::InvalidProcess)?
            .get(handle)
            .and_then(|c| *c)
            .ok_or(Error::InvalidHandle)?;
        if cap.rights != rights {
            return Err(Error::Denied);
        }
        Ok(cap.endpoint)
    }
    pub fn send(&mut self, pid: usize, handle: u64, word: u64) -> Result<(), Error> {
        let endpoint = self.endpoint(pid, handle, Rights::Send)?;
        self.queues[endpoint].send(Message { sender: pid, word })
    }
    pub fn receive(&mut self, pid: usize, handle: u64) -> Result<Message, Error> {
        let endpoint = self.endpoint(pid, handle, Rights::Receive)?;
        self.queues[endpoint].receive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn broker() -> Broker<2, 4, 2, 2> {
        let mut b = Broker::empty();
        b.grant(0, 0, 1, Rights::Send).unwrap();
        b.grant(0, 1, 0, Rights::Receive).unwrap();
        b.grant(1, 0, 0, Rights::Send).unwrap();
        b.grant(1, 1, 1, Rights::Receive).unwrap();
        b
    }
    #[test]
    fn kernel_assigns_sender_and_preserves_full_word() {
        let mut b = broker();
        b.send(0, 0, u64::MAX).unwrap();
        assert_eq!(
            b.receive(1, 1),
            Ok(Message {
                sender: 0,
                word: u64::MAX
            })
        );
        assert_eq!(b.receive(1, 1), Err(Error::Empty));
    }
    #[test]
    fn process_cannot_forge_endpoint_or_use_wrong_rights() {
        let mut b = broker();
        assert_eq!(b.send(0, 1, 7), Err(Error::Denied));
        assert_eq!(b.receive(0, 0), Err(Error::Denied));
        assert_eq!(b.send(0, 2, 7), Err(Error::InvalidHandle));
        assert_eq!(b.send(0, u64::MAX, 7), Err(Error::InvalidHandle));
        assert_eq!(b.send(9, 0, 7), Err(Error::InvalidProcess));
        assert_eq!(b.receive(1, 1), Err(Error::Empty));
    }
    #[test]
    fn full_queue_preserves_fifo_and_can_wrap() {
        let mut b = broker();
        b.send(0, 0, 10).unwrap();
        b.send(0, 0, 11).unwrap();
        assert_eq!(b.send(0, 0, 12), Err(Error::Full));
        assert_eq!(b.receive(1, 1).unwrap().word, 10);
        b.send(0, 0, 12).unwrap();
        assert_eq!(b.receive(1, 1).unwrap().word, 11);
        assert_eq!(b.receive(1, 1).unwrap().word, 12);
    }
    #[test]
    fn grants_cannot_silently_replace_existing_authority() {
        let mut b = broker();
        assert_eq!(b.grant(0, 0, 0, Rights::Receive), Err(Error::Occupied));
        assert_eq!(b.grant(0, 3, 9, Rights::Send), Err(Error::InvalidEndpoint));
        b.send(0, 0, 42).unwrap();
        assert_eq!(b.receive(1, 1).unwrap().word, 42);
    }
}
