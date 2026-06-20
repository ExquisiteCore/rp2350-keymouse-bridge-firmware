pub const LED_TICK_MS: u64 = 1;
pub const LED_MAX_BRIGHTNESS: u8 = 16;
pub const LED_MODE_DISCONNECTED: u8 = 0;
pub const LED_MODE_CONNECTED: u8 = 1;
pub const LED_SIGNAL_NONE: u8 = 0;
pub const LED_SIGNAL_ACTIVITY: u8 = 1;
pub const LED_SIGNAL_ERROR: u8 = 2;

pub const ACTIVITY_TICKS: u16 = 90;
pub const ERROR_TOTAL_TICKS: u16 = 280;

const BREATH_PERIOD_TICKS: u16 = 1280;
const HEARTBEAT_PERIOD_TICKS: u16 = 1500;
const HEARTBEAT_FIRST_ON_END: u16 = 80;
const HEARTBEAT_SECOND_ON_START: u16 = 160;
const HEARTBEAT_SECOND_ON_END: u16 = 240;
const CONNECTED_IDLE_BRIGHTNESS: u8 = 1;
const ERROR_ON_TICKS: u16 = 40;
const ERROR_STEP_TICKS: u16 = 90;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedMode {
    Disconnected,
    Connected,
}

impl LedMode {
    pub const fn from_u8(value: u8) -> Self {
        match value {
            LED_MODE_CONNECTED => Self::Connected,
            _ => Self::Disconnected,
        }
    }

    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Disconnected => LED_MODE_DISCONNECTED,
            Self::Connected => LED_MODE_CONNECTED,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedSignal {
    None,
    Activity,
    Error,
}

impl LedSignal {
    pub const fn from_u8(value: u8) -> Self {
        match value {
            LED_SIGNAL_ACTIVITY => Self::Activity,
            LED_SIGNAL_ERROR => Self::Error,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LedEvent {
    None,
    Activity,
    Error,
}

pub struct LedAnimator {
    mode: LedMode,
    mode_tick: u16,
    event: LedEvent,
    event_tick: u16,
    pwm_tick: u8,
}

impl LedAnimator {
    pub const fn new(mode: LedMode) -> Self {
        Self {
            mode,
            mode_tick: 0,
            event: LedEvent::None,
            event_tick: 0,
            pwm_tick: 0,
        }
    }

    pub fn set_mode(&mut self, mode: LedMode) {
        if self.mode != mode {
            self.mode = mode;
            self.mode_tick = 0;
        }
    }

    pub fn signal(&mut self, signal: LedSignal) {
        match signal {
            LedSignal::None => {}
            LedSignal::Activity if self.event != LedEvent::Error => {
                self.event = LedEvent::Activity;
                self.event_tick = 0;
            }
            LedSignal::Activity => {}
            LedSignal::Error => {
                self.event = LedEvent::Error;
                self.event_tick = 0;
            }
        }
    }

    pub fn next_brightness(&mut self) -> u8 {
        let brightness = match self.event {
            LedEvent::None => self.mode_brightness(),
            LedEvent::Activity => {
                if self.event_tick < ACTIVITY_TICKS {
                    LED_MAX_BRIGHTNESS
                } else {
                    self.event = LedEvent::None;
                    self.event_tick = 0;
                    self.mode_brightness()
                }
            }
            LedEvent::Error => {
                if self.event_tick >= ERROR_TOTAL_TICKS {
                    self.event = LedEvent::None;
                    self.event_tick = 0;
                    self.mode_brightness()
                } else if error_is_on(self.event_tick) {
                    LED_MAX_BRIGHTNESS
                } else {
                    0
                }
            }
        };

        self.advance_ticks();
        brightness
    }

    pub fn next_output(&mut self) -> bool {
        let brightness = self.next_brightness();
        let output = self.pwm_tick < brightness;
        self.pwm_tick = (self.pwm_tick + 1) % LED_MAX_BRIGHTNESS;
        output
    }

    fn mode_brightness(&self) -> u8 {
        match self.mode {
            LedMode::Disconnected => breath_brightness(self.mode_tick),
            LedMode::Connected => heartbeat_brightness(self.mode_tick),
        }
    }

    fn advance_ticks(&mut self) {
        self.mode_tick = self.mode_tick.wrapping_add(1);
        if self.event != LedEvent::None {
            self.event_tick = self.event_tick.wrapping_add(1);
        }
    }
}

fn breath_brightness(tick: u16) -> u8 {
    let phase = tick % BREATH_PERIOD_TICKS;
    let half = BREATH_PERIOD_TICKS / 2;
    let raw = if phase < half {
        phase
    } else {
        BREATH_PERIOD_TICKS - 1 - phase
    };

    ((raw as u32 * LED_MAX_BRIGHTNESS as u32) / (half as u32 - 1)) as u8
}

fn heartbeat_brightness(tick: u16) -> u8 {
    let phase = tick % HEARTBEAT_PERIOD_TICKS;
    if phase < HEARTBEAT_FIRST_ON_END
        || (HEARTBEAT_SECOND_ON_START..HEARTBEAT_SECOND_ON_END).contains(&phase)
    {
        LED_MAX_BRIGHTNESS
    } else {
        CONNECTED_IDLE_BRIGHTNESS
    }
}

fn error_is_on(tick: u16) -> bool {
    (tick % ERROR_STEP_TICKS) < ERROR_ON_TICKS && tick < ERROR_STEP_TICKS * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_mode_breathes_up_and_down() {
        let mut led = LedAnimator::new(LedMode::Disconnected);
        let samples = sample_brightness(&mut led, 1500);

        assert!(samples.iter().any(|level| *level == 0));
        assert!(samples.iter().any(|level| *level >= LED_MAX_BRIGHTNESS - 1));
        assert!(samples.windows(2).any(|pair| pair[1] > pair[0]));
        assert!(samples.windows(2).any(|pair| pair[1] < pair[0]));
    }

    #[test]
    fn connected_mode_uses_double_heartbeat() {
        let mut led = LedAnimator::new(LedMode::Connected);
        let samples = sample_brightness(&mut led, 260);
        let bright_runs = count_bright_runs(&samples);

        assert_eq!(bright_runs, 2);
    }

    #[test]
    fn activity_signal_overrides_idle_briefly() {
        let mut led = LedAnimator::new(LedMode::Connected);
        led.signal(LedSignal::Activity);
        let samples = sample_brightness(&mut led, 130);

        assert!(
            samples[..ACTIVITY_TICKS as usize]
                .iter()
                .all(|level| *level == LED_MAX_BRIGHTNESS)
        );
        assert!(
            samples[ACTIVITY_TICKS as usize..]
                .iter()
                .any(|level| *level < LED_MAX_BRIGHTNESS)
        );
    }

    #[test]
    fn error_signal_uses_three_fast_blinks() {
        let mut led = LedAnimator::new(LedMode::Connected);
        led.signal(LedSignal::Error);
        let samples = sample_brightness(&mut led, ERROR_TOTAL_TICKS as usize);
        let bright_runs = count_bright_runs(&samples);

        assert_eq!(bright_runs, 3);
    }

    fn sample_brightness(led: &mut LedAnimator, count: usize) -> [u8; 1600] {
        let mut out = [0u8; 1600];
        for slot in out.iter_mut().take(count) {
            *slot = led.next_brightness();
        }
        out
    }

    fn count_bright_runs(samples: &[u8]) -> usize {
        let mut runs = 0usize;
        let mut was_bright = false;
        for sample in samples {
            let bright = *sample == LED_MAX_BRIGHTNESS;
            if bright && !was_bright {
                runs += 1;
            }
            was_bright = bright;
        }
        runs
    }
}
