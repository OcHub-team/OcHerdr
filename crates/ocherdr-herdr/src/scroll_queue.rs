//! Coalesce contiguous wheel input before it reaches the socket. Keep the first
//! event immediate, then send at most once per 8 ms while scrolling steadily.
//! Input, resize, release and direction changes are ordering barriers, not sums:
//! opposite scrolls cannot cancel at a scrollback boundary or inside a mouse TUI.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{TerminalCommand, TerminalScrollDirection};

const SCROLL_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Default)]
struct State {
    commands: VecDeque<TerminalCommand>,
    closed: bool,
    next_scroll: Option<(TerminalScrollDirection, Instant)>,
}

#[derive(Default)]
struct Queue {
    state: Mutex<State>,
    ready: Condvar,
}

pub(crate) struct CommandSender(Arc<Queue>);
pub(crate) struct CommandReceiver(Arc<Queue>);

pub(crate) fn channel() -> (CommandSender, CommandReceiver) {
    let queue = Arc::new(Queue::default());
    (CommandSender(queue.clone()), CommandReceiver(queue))
}

impl State {
    fn push(&mut self, mut command: TerminalCommand) {
        if let TerminalCommand::Scroll { lines, .. } = &mut command {
            *lines = (*lines).max(1);
        }
        if let TerminalCommand::Scroll { direction, lines } = &command
            && let Some(TerminalCommand::Scroll {
                direction: previous_direction,
                lines: previous_lines,
            }) = self.commands.back_mut()
            && direction == previous_direction
            && let Some(sum) = previous_lines.checked_add((*lines).max(1))
        {
            *previous_lines = sum;
            return;
        }
        self.commands.push_back(command);
    }

    fn delay(&self, now: Instant) -> Option<Duration> {
        // Anything following the leading scroll is a barrier (or an overflow
        // chunk). Flush it now so keys and direction reversals never wait.
        if self.commands.len() == 1
            && let Some(TerminalCommand::Scroll { direction, .. }) = self.commands.front()
            && let Some((previous, next)) = self.next_scroll
            && *direction == previous
        {
            next.checked_duration_since(now)
                .filter(|delay| !delay.is_zero())
        } else {
            None
        }
    }

    fn pop(&mut self, now: Instant) -> Option<TerminalCommand> {
        let command = self.commands.pop_front()?;
        self.next_scroll = match command {
            TerminalCommand::Scroll { direction, .. } => Some((direction, now + SCROLL_INTERVAL)),
            _ => None,
        };
        Some(command)
    }
}

impl CommandSender {
    pub(crate) fn send(&self, command: TerminalCommand) -> Result<(), ()> {
        let mut state = self.0.state.lock().unwrap();
        if state.closed {
            return Err(());
        }
        state.push(command);
        self.0.ready.notify_one();
        Ok(())
    }
}

impl Drop for CommandSender {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().closed = true;
        self.0.ready.notify_one();
    }
}

impl CommandReceiver {
    pub(crate) fn recv(&self) -> Option<TerminalCommand> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if !state.commands.is_empty() {
                let now = Instant::now();
                if !state.closed
                    && let Some(delay) = state.delay(now)
                {
                    state = self.0.ready.wait_timeout(state, delay).unwrap().0;
                    continue;
                }
                return state.pop(now);
            }
            if state.closed {
                return None;
            }
            state = self.0.ready.wait(state).unwrap();
        }
    }
}

impl Drop for CommandReceiver {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.closed = true;
        state.commands.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalScrollDirection::{Down, Up};

    fn scroll(lines: u16) -> TerminalCommand {
        TerminalCommand::Scroll {
            direction: Up,
            lines,
        }
    }

    #[test]
    fn burst_is_coalesced_without_delaying_first_event() {
        let mut state = State::default();
        let start = Instant::now();
        state.push(scroll(1));
        assert_eq!(state.delay(start), None);
        assert_eq!(state.pop(start), Some(scroll(1)));
        for _ in 0..120 {
            state.push(scroll(1));
        }
        assert_eq!(state.commands.len(), 1);
        assert_eq!(state.delay(start), Some(SCROLL_INTERVAL));
        assert_eq!(state.delay(start + SCROLL_INTERVAL), None);
        assert_eq!(state.pop(start + SCROLL_INTERVAL), Some(scroll(120)));
    }

    #[test]
    fn reversal_keys_resize_and_release_keep_order_and_flush_immediately() {
        for barrier in [
            TerminalCommand::Scroll {
                direction: Down,
                lines: 3,
            },
            TerminalCommand::Input(b"x".to_vec()),
            TerminalCommand::Resize {
                cols: 80,
                rows: 24,
                cell_width_px: 8,
                cell_height_px: 16,
            },
            TerminalCommand::Release,
        ] {
            let mut state = State::default();
            let now = Instant::now();
            state.push(scroll(1));
            state.pop(now);
            state.push(scroll(2));
            state.push(barrier.clone());
            assert_eq!(state.delay(now), None);
            assert_eq!(state.pop(now), Some(scroll(2)));
            assert_eq!(state.delay(now), None);
            assert_eq!(state.pop(now), Some(barrier));
        }
    }

    #[test]
    fn overflow_is_split_and_sender_drop_drains_release() {
        let (tx, rx) = channel();
        tx.send(scroll(u16::MAX)).unwrap();
        tx.send(scroll(2)).unwrap();
        tx.send(TerminalCommand::Release).unwrap();
        drop(tx);
        assert_eq!(rx.recv(), Some(scroll(u16::MAX)));
        assert_eq!(rx.recv(), Some(scroll(2)));
        assert_eq!(rx.recv(), Some(TerminalCommand::Release));
        assert_eq!(rx.recv(), None);
    }

    #[test]
    fn disconnected_writer_rejects_commands() {
        let (tx, rx) = channel();
        drop(rx);
        assert!(tx.send(scroll(1)).is_err());
    }
}
