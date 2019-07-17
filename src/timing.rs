use nb;
use void::Void;

// TODO: Is there a way to make Timer unimplementable without `CountDown
/**
  A countdown timer which nonblockingly waits until the specified countdown
  is completed. The countdown is started by calling `start` from the CountDown trait
*/
pub trait Timer {
    /**
      Returns Err(WouldBlock) if less time than `delay` has passed since `start_real`
      was called and `Ok(())` if more time has passed.

      If start_real hasn't been called yet, the behaviour is undefined
    */
    fn wait(&mut self) -> nb::Result<(), Void>;
}

pub trait CountDown<Unit> : Timer {
    /**
      Set the timer to the specified delay and start the count down
    */
    fn start(&mut self, delay: Unit);
}


#[derive(Clone, Copy)]
pub struct Second(pub u32);
#[derive(Clone, Copy)]
pub struct Millisecond(pub u32);
#[derive(Clone, Copy)]
pub struct Microsecond(pub u32);

// Conversions for `Milliseconds`
impl From<Second> for Millisecond {
    fn from(other: Second) -> Self {
        Millisecond(other.0 * 1000)
    }
}

// Conversions for `Microsecond`
impl From<Millisecond> for Microsecond {
    fn from(other: Millisecond) -> Self {
        Microsecond(other.0 * 1000)
    }
}
impl From<Second> for Microsecond {
    fn from(other: Second) -> Self {
        Microsecond(other.0 * 1_000_000)
    }
}
