//! The push under a lease, against a fake server. The rules: an unmarked session is never sent;
//! the write happens under the granted lease; the lease is always given back, even when the
//! write fails; and a held conversation is refused without writing.
use sesync::state::State;
use sesync::team_push::{push_marked, PushError, TeamRemote};
use std::cell::RefCell;

#[derive(Default)]
struct Fake {
    acquire_result: RefCell<Option<Result<u64, PushError>>>,
    put_result: RefCell<Option<Result<(), PushError>>>,
    log: RefCell<Vec<String>>,
}

impl TeamRemote for Fake {
    fn acquire(&self, _s: &str, c: &str) -> Result<u64, PushError> {
        self.log.borrow_mut().push(format!("acquire {c}"));
        self.acquire_result.borrow_mut().take().unwrap_or(Ok(1))
    }
    fn put(&self, _s: &str, c: &str, v: u64, b: &[u8]) -> Result<(), PushError> {
        self.log.borrow_mut().push(format!("put {c} v{v} {}b", b.len()));
        self.put_result.borrow_mut().take().unwrap_or(Ok(()))
    }
    fn release(&self, _s: &str, c: &str) -> Result<(), PushError> {
        self.log.borrow_mut().push(format!("release {c}"));
        Ok(())
    }
}

fn marked(id: &str) -> State {
    let mut st = State::default();
    st.mark(id);
    st
}

#[test]
fn a_marked_session_is_written_under_its_lease_then_released() {
    let f = Fake::default();
    push_marked(&marked("ses_1"), &f, "personal", "ses_1", b"hello").unwrap();
    assert_eq!(*f.log.borrow(), vec!["acquire ses_1", "put ses_1 v1 5b", "release ses_1"]);
}

#[test]
fn an_unmarked_session_is_never_sent() {
    let f = Fake::default();
    let r = push_marked(&State::default(), &f, "personal", "ses_1", b"secret");
    assert!(matches!(r, Err(PushError::Refused(_))));
    assert!(f.log.borrow().is_empty(), "no request at all for an unmarked session");
}

#[test]
fn a_held_conversation_is_refused_and_nothing_is_written() {
    let f = Fake::default();
    *f.acquire_result.borrow_mut() = Some(Err(PushError::Held));
    let r = push_marked(&marked("ses_1"), &f, "personal", "ses_1", b"x");
    assert_eq!(r, Err(PushError::Held));
    assert_eq!(*f.log.borrow(), vec!["acquire ses_1"], "no write and no release for a lease never granted");
}

#[test]
fn the_lease_is_released_even_when_the_write_fails() {
    let f = Fake::default();
    *f.put_result.borrow_mut() = Some(Err(PushError::Unreachable));
    let r = push_marked(&marked("ses_1"), &f, "personal", "ses_1", b"x");
    assert_eq!(r, Err(PushError::Unreachable));
    assert_eq!(f.log.borrow().last().unwrap(), "release ses_1", "a failed write still gives the lease back");
}
