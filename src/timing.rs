use nb;
use core::convert::Infallible;

/**
    A countdown timer which nonblockingly waits until the specified countdown
    is completed. The countdown is started by calling `start`.

    This trait is needed because `embedded_hal` currently does not have a standardised
    measure of time.

    The implementation of this trait will depend on your HAL implementation, but
    here is a sample impl for the stm32f1xx_hal


    ```rust
    struct LongTimer<T> {
        timer: Timer<T>,
        milliseconds_remaining: u32,
    }

    impl<T> LongTimer<T>
        where Timer<T>: CountDown<Time = Hertz> + Periodic
    {
        pub fn new(timer: Timer<T>) -> Self {
            Self {
                timer,
                milliseconds_remaining: 0
            }
        }
        fn process_tick(&mut self) -> nb::Result<(), Infallible>{
            match self.milliseconds_remaining {
                0 => Ok(()),
                t @ 0..=1000 => {
                    self.milliseconds_remaining = 0;
                    self.timer.start(((1000. / t as f32) as u32).hz());
                    Err(nb::Error::WouldBlock)
                }
                _ => {
                    self.timer.start(1.hz());
                    self.milliseconds_remaining -= 1000;
                    Err(nb::Error::WouldBlock)
                }
            }
        }
    }

    impl<T> esp01::LongTimer for LongTimer<T>
        where Timer<T>: CountDown<Time = Hertz> + Periodic
    {
        fn wait(&mut self) -> nb::Result<(), Infallible> {
            match self.timer.wait() {
                Ok(_) => self.process_tick(),
                Err(nb::Error::WouldBlock) => Err(nb::Error::WouldBlock),
                Err(_void) => unreachable!() // The void type can not exist
            }
        }
        fn start(&mut self, Millisecond(duration): Millisecond) {
            self.milliseconds_remaining = duration;
            // This will always return nb::WouldBlock (unless duration is 0)
            self.process_tick().ok();
        }

    }
    ```
*/
pub trait LongTimer {
    /**
      Returns Err(WouldBlock) if less time than `delay` has passed since `start_real`
      was called and `Ok(())` if more time has passed.

      If start_real hasn't been called yet, the behaviour is undefined
    */
    fn wait(&mut self) -> nb::Result<(), Infallible>;

    /**
        Start the count down for the specified amount of milliseconds.

        The required accuracy depends on the duration. Above 1 second, the
        requirements are very low.
    */
    fn start(&mut self, duration: Millisecond);
}


#[derive(Clone, Copy)]
pub struct Second(pub u32);
#[derive(Clone, Copy)]
pub struct Millisecond(pub u32);

impl From<Second> for Millisecond {
    fn from(Second(duration): Second) -> Self {
        return Millisecond(duration * 1000);
    }
}

