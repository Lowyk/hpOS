#![no_std]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Unused,
    Ready,
    Running,
    Sleeping(u64),
    Stopped,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidThread,
    Occupied,
    IdleCannotSleep,
    InvalidDuration,
}

/// Single-CPU scheduling policy. Hardware context storage is supplied by the kernel.
pub struct RoundRobin<const N: usize> {
    states: [State; N],
    current: usize,
    idle: usize,
    ticks: u64,
    elapsed: u64,
    quantum: u64,
    switches: u64,
}
impl<const N: usize> RoundRobin<N> {
    pub const fn new(idle: usize, quantum: u64) -> Self {
        assert!(idle < N && quantum > 0);
        let mut states = [State::Unused; N];
        states[idle] = State::Running;
        Self {
            states,
            current: idle,
            idle,
            ticks: 0,
            elapsed: 0,
            quantum,
            switches: 0,
        }
    }
    pub fn add(&mut self, id: usize) -> Result<(), Error> {
        let state = self.states.get_mut(id).ok_or(Error::InvalidThread)?;
        if *state != State::Unused {
            return Err(Error::Occupied);
        }
        *state = State::Ready;
        Ok(())
    }
    pub fn start(&mut self, id: usize) -> Result<(), Error> {
        if self.states.get(id) != Some(&State::Ready) {
            return Err(Error::InvalidThread);
        }
        self.states[self.current] = State::Ready;
        self.states[id] = State::Running;
        self.current = id;
        self.elapsed = 0;
        Ok(())
    }
    pub fn current(&self) -> usize {
        self.current
    }
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
    pub fn switches(&self) -> u64 {
        self.switches
    }
    pub fn state(&self, id: usize) -> Option<State> {
        self.states.get(id).copied()
    }

    pub fn sleep_current(&mut self, duration: u64) -> Result<(), Error> {
        if self.current == self.idle {
            return Err(Error::IdleCannotSleep);
        }
        if duration >= 1 << 63 {
            return Err(Error::InvalidDuration);
        }
        if duration != 0 {
            self.states[self.current] = State::Sleeping(self.ticks.wrapping_add(duration));
        }
        Ok(())
    }
    pub fn on_tick(&mut self) -> usize {
        self.ticks = self.ticks.wrapping_add(1);
        for state in &mut self.states {
            if let State::Sleeping(deadline) = *state {
                if self.ticks.wrapping_sub(deadline) < 1 << 63 {
                    *state = State::Ready;
                }
            }
        }
        self.elapsed = self.elapsed.saturating_add(1);
        if self.current == self.idle
            || self.states[self.current] != State::Running
            || self.elapsed >= self.quantum
        {
            self.reschedule()
        } else {
            self.current
        }
    }
    pub fn stop_current(&mut self) -> Result<(), Error> {
        if self.current == self.idle {
            return Err(Error::IdleCannotSleep);
        }
        self.states[self.current] = State::Stopped;
        Ok(())
    }
    pub fn reschedule(&mut self) -> usize {
        let previous = self.current;
        if self.states[previous] == State::Running {
            self.states[previous] = State::Ready;
        }
        let mut next = self.idle;
        for step in 1..=N {
            let id = (previous + step) % N;
            if id != self.idle && self.states[id] == State::Ready {
                next = id;
                break;
            }
        }
        self.states[next] = State::Running;
        self.current = next;
        self.elapsed = 0;
        if next != previous {
            self.switches = self.switches.wrapping_add(1);
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> RoundRobin<4> {
        let mut p = RoundRobin::new(3, 2);
        for id in 0..3 {
            p.add(id).unwrap();
        }
        p.start(0).unwrap();
        p
    }
    #[test]
    fn non_yielding_threads_rotate_after_quantum_without_running_idle() {
        let mut p = policy();
        for expected in [0, 1, 1, 2, 2, 0, 0, 1, 1, 2, 2, 0] {
            assert_eq!(p.on_tick(), expected);
        }
        assert_eq!(p.switches(), 6);
    }
    #[test]
    fn sleeping_threads_are_skipped_and_woken_on_deadline() {
        let mut p = policy();
        p.sleep_current(3).unwrap();
        assert_eq!(p.reschedule(), 1);
        assert_eq!(p.on_tick(), 1);
        assert_eq!(p.on_tick(), 2);
        assert_eq!(p.state(0), Some(State::Sleeping(3)));
        assert_eq!(p.on_tick(), 2);
        assert_eq!(p.state(0), Some(State::Ready));
        assert_eq!(p.on_tick(), 0);
    }
    #[test]
    fn idle_runs_only_until_work_becomes_ready() {
        let mut p = policy();
        for _ in 0..3 {
            p.sleep_current(5).unwrap();
            p.reschedule();
        }
        assert_eq!(p.current(), 3);
        for _ in 0..4 {
            assert_eq!(p.on_tick(), 3);
        }
        assert_ne!(p.on_tick(), 3);
    }
    #[test]
    fn deadlines_survive_tick_counter_wrap() {
        let mut p = policy();
        p.ticks = u64::MAX - 1;
        p.sleep_current(3).unwrap();
        p.reschedule();
        p.on_tick();
        p.on_tick();
        assert_eq!(p.state(0), Some(State::Sleeping(1)));
        p.on_tick();
        assert_eq!(p.state(0), Some(State::Ready));
    }
    #[test]
    fn invalid_registration_and_sleep_do_not_change_state() {
        let mut p = policy();
        assert_eq!(p.add(0), Err(Error::Occupied));
        assert_eq!(p.add(9), Err(Error::InvalidThread));
        assert_eq!(p.sleep_current(1 << 63), Err(Error::InvalidDuration));
        assert_eq!(p.state(0), Some(State::Running));
        let mut idle = RoundRobin::<1>::new(0, 2);
        assert_eq!(idle.sleep_current(1), Err(Error::IdleCannotSleep));
    }
    #[test]
    fn single_runnable_thread_keeps_running_without_fake_switches() {
        let mut p = RoundRobin::<2>::new(1, 2);
        p.add(0).unwrap();
        p.start(0).unwrap();
        for _ in 0..100 {
            assert_eq!(p.on_tick(), 0);
        }
        assert_eq!(p.switches(), 0);
    }
    #[test]
    fn stopped_process_is_never_woken_or_selected() {
        let mut p = policy();
        p.reschedule();
        assert_eq!(p.current(), 1);
        p.stop_current().unwrap();
        assert_eq!(p.reschedule(), 2);
        for _ in 0..100 {
            assert_ne!(p.on_tick(), 1);
        }
        assert_eq!(p.state(1), Some(State::Stopped));
    }
}
