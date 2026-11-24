//! Android touch gestures, in surface pixels and monotonic event time.
//! The recognizer emits the same tool input as the web (SPEC §3.5–3.6).

use std::time::Duration;

use desktank_sim::tools::Button;

type Point = (f64, f64);
type PointerId = i32;

const LONG_PRESS: Duration = Duration::from_millis(500);
const TOUCH_SLOP: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchEvent {
    Down {
        id: PointerId,
        point: Point,
        density: f64,
    },
    Move {
        id: PointerId,
        point: Point,
    },
    Up(PointerId),
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchAction {
    Press(Button, Point),
    Move(Point),
    Release(Button),
    Leave,
    InspectAt(Point),
}

#[derive(Debug, Default)]
enum Mode {
    Pending {
        start: Point,
        slop: f64,
        deadline: Duration,
    },
    Stroke,
    Scoop,
    #[default]
    Done,
}

#[derive(Debug, Default)]
pub struct Touch {
    // In order of arrival: the first two fingers define the scoop midpoint.
    fingers: Vec<(PointerId, Point)>,
    mode: Mode,
}

impl Touch {
    pub fn deadline(&self) -> Option<Duration> {
        match self.mode {
            Mode::Pending { deadline, .. } => Some(deadline),
            _ => None,
        }
    }

    pub fn advance(&mut self, now: Duration, emit: impl FnOnce(TouchAction)) {
        if let Mode::Pending {
            start, deadline, ..
        } = self.mode
            && now >= deadline
        {
            self.mode = Mode::Done;
            emit(TouchAction::InspectAt(start));
        }
    }

    pub fn handle(&mut self, event: TouchEvent, at: Duration, mut emit: impl FnMut(TouchAction)) {
        if event == TouchEvent::Cancel {
            self.cancel(emit);
            return;
        }
        // An input event may be queued across the deadline while the render
        // thread is busy. Resolve it in event-time order.
        self.advance(at, &mut emit);
        match event {
            TouchEvent::Down { id, point, density } => self.down(id, point, density, at, emit),
            TouchEvent::Move { id, point } => self.moved(id, point, emit),
            TouchEvent::Up(id) => self.up(id, emit),
            TouchEvent::Cancel => {}
        }
    }

    fn down(
        &mut self,
        id: PointerId,
        point: Point,
        density: f64,
        at: Duration,
        mut emit: impl FnMut(TouchAction),
    ) {
        if self.fingers.iter().any(|&(finger, _)| finger == id) {
            return;
        }
        self.fingers.push((id, point));
        match self.fingers.as_slice() {
            [_] => {
                self.mode = Mode::Pending {
                    start: point,
                    slop: TOUCH_SLOP * density,
                    deadline: at + LONG_PRESS,
                };
            }
            [(_, a), (_, b)] if matches!(self.mode, Mode::Pending { .. } | Mode::Stroke) => {
                if matches!(self.mode, Mode::Stroke) {
                    emit(TouchAction::Release(Button::Primary));
                }
                self.mode = Mode::Scoop;
                emit(TouchAction::Press(Button::Secondary, midpoint(*a, *b)));
            }
            _ => {}
        }
    }

    fn moved(&mut self, id: PointerId, point: Point, mut emit: impl FnMut(TouchAction)) {
        let Some((_, current)) = self.fingers.iter_mut().find(|(finger, _)| *finger == id) else {
            return;
        };
        *current = point;
        if let Mode::Pending { start, slop, .. } = self.mode
            && (point.0 - start.0).hypot(point.1 - start.1) >= slop
        {
            self.mode = Mode::Stroke;
            emit(TouchAction::Press(Button::Primary, start));
        }
        match self.mode {
            Mode::Stroke => emit(TouchAction::Move(point)),
            Mode::Scoop => {
                if let [(_, a), (_, b), ..] = self.fingers.as_slice() {
                    emit(TouchAction::Move(midpoint(*a, *b)));
                }
            }
            _ => {}
        }
    }

    fn up(&mut self, id: PointerId, mut emit: impl FnMut(TouchAction)) {
        let Some(index) = self.fingers.iter().position(|&(finger, _)| finger == id) else {
            return;
        };
        self.fingers.remove(index);
        if let Mode::Pending { start, .. } = self.mode {
            emit(TouchAction::Press(Button::Primary, start));
            emit(TouchAction::Release(Button::Primary));
        } else {
            self.end_stroke(&mut emit);
        }
        self.mode = Mode::Done;
        if self.fingers.is_empty() {
            emit(TouchAction::Leave);
        }
    }

    pub fn cancel(&mut self, mut emit: impl FnMut(TouchAction)) {
        if !self.fingers.is_empty() {
            self.end_stroke(&mut emit);
            self.mode = Mode::Done;
            self.fingers.clear();
            emit(TouchAction::Leave);
        }
    }

    fn end_stroke(&self, mut emit: impl FnMut(TouchAction)) {
        match self.mode {
            Mode::Stroke => emit(TouchAction::Release(Button::Primary)),
            Mode::Scoop => emit(TouchAction::Release(Button::Secondary)),
            _ => {}
        }
    }
}

fn midpoint(a: Point, b: Point) -> Point {
    (f64::midpoint(a.0, b.0), f64::midpoint(a.1, b.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Button::{Primary, Secondary};
    use TouchAction::{InspectAt, Leave, Move, Press, Release};

    const START: Point = (20.0, 40.0);

    fn send(touch: &mut Touch, event: TouchEvent, ms: u64) -> Vec<TouchAction> {
        let mut actions = Vec::new();
        touch.handle(event, Duration::from_millis(ms), |a| actions.push(a));
        actions
    }

    fn down(id: PointerId, point: Point) -> TouchEvent {
        TouchEvent::Down {
            id,
            point,
            density: 2.0,
        }
    }

    fn moved(id: PointerId, point: Point) -> TouchEvent {
        TouchEvent::Move { id, point }
    }

    #[test]
    fn a_tap_stamps_once_at_the_start_and_leaves() {
        let mut touch = Touch::default();
        assert_eq!(send(&mut touch, down(7, START), 100), []);
        assert_eq!(touch.deadline(), Some(Duration::from_millis(600)));
        assert_eq!(send(&mut touch, moved(7, (29.0, 49.0)), 200), []);
        assert_eq!(
            send(&mut touch, TouchEvent::Up(7), 300),
            [Press(Primary, START), Release(Primary), Leave]
        );
        assert_eq!(touch.deadline(), None);
    }

    #[test]
    fn crossing_the_density_scaled_slop_starts_a_stroke_at_the_start() {
        let mut touch = Touch::default();
        send(&mut touch, down(7, START), 100);
        assert_eq!(send(&mut touch, moved(7, (35.0, 40.0)), 200), []);
        let edge = (36.0, 40.0);
        assert_eq!(
            send(&mut touch, moved(7, edge), 599),
            [Press(Primary, START), Move(edge)]
        );
        assert_eq!(touch.deadline(), None);
        assert_eq!(send(&mut touch, moved(7, START), 700), [Move(START)]);
        assert_eq!(
            send(&mut touch, TouchEvent::Up(7), 750),
            [Release(Primary), Leave]
        );
    }

    #[test]
    fn long_press_inspects_once_and_suppresses_strokes_until_all_fingers_lift() {
        let mut touch = Touch::default();
        send(&mut touch, down(7, START), 100);
        send(&mut touch, moved(7, (22.0, 40.0)), 300);
        let mut actions = Vec::new();
        touch.advance(Duration::from_millis(599), |a| actions.push(a));
        assert_eq!(actions, []);
        touch.advance(Duration::from_millis(600), |a| actions.push(a));
        touch.advance(Duration::from_millis(601), |a| actions.push(a));
        assert_eq!(actions, [InspectAt(START)]);
        assert_eq!(touch.deadline(), None);
        assert_eq!(send(&mut touch, moved(7, (70.0, 40.0)), 650), []);
        assert_eq!(send(&mut touch, down(2, (80.0, 40.0)), 680), []);
        assert_eq!(send(&mut touch, TouchEvent::Up(7), 700), []);
        assert_eq!(send(&mut touch, TouchEvent::Up(2), 710), [Leave]);
    }

    #[test]
    fn an_up_queued_past_the_deadline_is_a_long_press() {
        let mut touch = Touch::default();
        send(&mut touch, down(7, START), 100);
        assert_eq!(
            send(&mut touch, TouchEvent::Up(7), 600),
            [InspectAt(START), Leave]
        );
    }

    #[test]
    fn a_second_finger_replaces_a_pending_gesture_or_a_stroke_with_a_scoop() {
        for dragging in [false, true] {
            let mut touch = Touch::default();
            send(&mut touch, down(7, START), 100);
            let first = if dragging { (40.0, 60.0) } else { (22.0, 40.0) };
            send(&mut touch, moved(7, first), 200);
            let second = (60.0, 80.0);
            let centre = if dragging { (50.0, 70.0) } else { (41.0, 60.0) };
            let mut expected = Vec::new();
            if dragging {
                expected.push(Release(Primary));
            }
            expected.push(Press(Secondary, centre));
            assert_eq!(send(&mut touch, down(2, second), 250), expected);
            assert_eq!(touch.deadline(), None);
            assert_eq!(
                send(&mut touch, moved(2, (70.0, 80.0)), 700),
                [Move((centre.0 + 5.0, centre.1))]
            );
            // Extra fingers don't change the midpoint. Any lift ends the scoop.
            assert_eq!(send(&mut touch, down(4, START), 725), []);
            assert_eq!(
                send(&mut touch, TouchEvent::Up(4), 750),
                [Release(Secondary)]
            );
            assert_eq!(send(&mut touch, moved(7, START), 775), []);
            assert_eq!(send(&mut touch, TouchEvent::Up(2), 800), []);
            assert_eq!(send(&mut touch, TouchEvent::Up(7), 850), [Leave]);
        }
    }

    #[test]
    fn cancel_never_taps_and_releases_an_active_stroke() {
        let mut touch = Touch::default();
        assert_eq!(send(&mut touch, moved(7, START), 90), []);
        send(&mut touch, down(7, START), 100);
        assert_eq!(send(&mut touch, TouchEvent::Cancel, 599), [Leave]);
        assert_eq!(touch.deadline(), None);
        assert_eq!(send(&mut touch, TouchEvent::Up(7), 610), []);

        send(&mut touch, down(7, START), 620);
        send(&mut touch, down(2, (60.0, 80.0)), 630);
        assert_eq!(
            send(&mut touch, TouchEvent::Cancel, 640),
            [Release(Secondary), Leave]
        );
    }
}
