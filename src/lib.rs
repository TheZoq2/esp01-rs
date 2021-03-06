#![no_std]

use embedded_hal as hal;

use nb::block;

use core::cmp::min;
use core::fmt::{self};
use arrayvec::{CapacityError, ArrayString};
use itoa;

mod serial;
mod timing;

pub use timing::{LongTimer, Second, Millisecond};

/**
    Maximum length of an AT response (Length of message + CRLF)

    longest message: `WIFI GOT IP\r\n`
*/
const AT_RESPONSE_BUFFER_SIZE: usize = 13;

/**
  Possible responses from an esp8266 AT command.

  This does not contain all possible responses but it does contain
  ever response that can be received from the commands sent in this crates
*/
#[derive(Debug, PartialEq)]
pub enum ATResponse {
    Ok,
    Error,
    Busy,
    WiFiGotIp,
}

/**
  Error type for esp communication.

  `R` and `T` are the error types of the serial module
*/
#[derive(Debug)]
pub enum Error<R, T, P> {
    /// Serial transmission errors
    TxError(T),
    /// Serial reception errors
    RxError(R),
    // Digital pin errors
    PinError(P),
    /// Invalid or unexpected data received from the device
    UnexpectedResponse(ATResponse),
    /// Errors from the formating of messages
    Fmt(fmt::Error),
    /// Error indicating an ArrayString wasn't big enough
    Capacity(CapacityError)
}
impl<R,T, P> From<fmt::Error> for Error<R,T, P> {
    fn from(other: fmt::Error) -> Error<R,T, P> {
        Error::Fmt(other)
    }
}
impl<R,T,ErrType,P> From<CapacityError<ErrType>> for Error<R,T,P> {
    fn from(other: CapacityError<ErrType>) -> Error<R,T,P> {
        Error::Capacity(other.simplify())
    }
}

/**
    Indicates what step in the data transmission that the sensor is in. Used
    in `TransmissionError` for reporting information about where things went wrong
*/
#[derive(Debug)]
pub enum TransmissionStep {
    Connect,
    Send,
    Close
}
/**
  Error indicating failure to transmit a message.
*/
#[derive(Debug)]
pub struct TransmissionError<R, T, P> {
    step: TransmissionStep,
    cause: Error<R, T, P>
}

impl<R, T, P> TransmissionError<R, T, P> {
    pub fn try_step<RetType>(step: TransmissionStep, cause: Result<RetType, Error<R, T, P>>) 
        -> Result<RetType, Self>
    {
        cause.map_err(|e| {
            Self {
                step,
                cause: e
            }
        })
    }
}


pub enum ConnectionType {
    Tcp,
    Udp
}
impl ConnectionType {
    pub fn as_str(&self) -> &str {
        match *self {
            ConnectionType::Tcp => "TCP",
            ConnectionType::Udp => "UDP"
        }
    }
}


macro_rules! return_type {
    ($ok:ty) => {
        Result<$ok, Error<serial::Error<Rx::Error>, Tx::Error, Rst::Error>>
    }
}

macro_rules! transmission_return_type {
    ($ok:ty) => {
        Result<$ok, TransmissionError<serial::Error<Rx::Error>, Tx::Error, Rst::Error>>
    }
}


////////////////////////////////////////////////////////////////////////////////
////////////////////////////////////////////////////////////////////////////////

const STARTUP_TIMEOUT: Second = Second(10);
const DEFAULT_TIMEOUT: Second = Second(5);


/**
  Struct for interracting with an esp8266 wifi module over USART
*/
pub struct Esp8266<Tx, Rx, Timer, Rst>
where Tx: hal::serial::Write<u8>,
      Rx: hal::serial::Read<u8>,
      Timer: LongTimer,
      Rst: hal::digital::v2::OutputPin
{
    tx: Tx,
    rx: Rx,
    timer: Timer,
    chip_enable_pin: Rst
}

impl<Tx, Rx, Timer, Rst> Esp8266<Tx, Rx, Timer, Rst>
where Tx: hal::serial::Write<u8>,
      Rx: hal::serial::Read<u8>,
      Timer: LongTimer,
      Rst: hal::digital::v2::OutputPin,
{
    /**
      Sets up the esp8266 struct and configures the device for future use

      `tx` and `rx` are the pins used for serial communication, `timer` is
      a hardware timer for dealing with things like serial timeout and
      `chip_enable_pin` is a pin which must be connected to the CHIP_EN pin
      of the device
    */
    pub fn new(tx: Tx, rx: Rx, timer: Timer, chip_enable_pin: Rst)
        -> return_type!(Self)
    {
        let mut result = Self {tx, rx, timer, chip_enable_pin};

        result.reset()?;

        Ok(result)
    }

    pub fn send_data(
        &mut self,
        connection_type: ConnectionType,
        address: &str,
        port: u16,
        data: &str
    ) -> transmission_return_type!(())
    {
        // Send a start connection message
        let tcp_start_result = self.start_tcp_connection(connection_type, address, port);
        TransmissionError::try_step(TransmissionStep::Connect, tcp_start_result)?;

        TransmissionError::try_step(TransmissionStep::Send, self.transmit_data(data))?;

        TransmissionError::try_step(TransmissionStep::Close, self.close_connection())
    }

    pub fn close_connection(&mut self) -> return_type!(()) {
        self.send_at_command("+CIPCLOSE")?;
        self.wait_for_ok(DEFAULT_TIMEOUT.into())
    }

    /**
      Turns off the device by setting chip_enable to 0
    */
    pub fn power_down(&mut self) -> return_type!(()) {
        self.chip_enable_pin.set_low().map_err(Error::PinError)
    }

    /**
      Resets the device by setting chip_enable to 0 and then back to 1
    */
    pub fn reset(&mut self) -> return_type!(()) {
        self.power_down()?;
        self.timer.start(Millisecond(10));
        block!(self.timer.wait()).unwrap();
        self.power_up()
    }

    /**
      Turns the device back on by setting chip_enable to high
    */
    pub fn power_up(&mut self) -> return_type!(()) {
        self.chip_enable_pin.set_high().map_err(Error::PinError)?;

        // The esp01 sends a bunch of garbage over the serial port before starting properly,
        // therefore we need to retry this until we get valid data or time out
        let mut error_count = 0;
        loop {
            match self.wait_for_got_ip(STARTUP_TIMEOUT.into()) {
                Ok(()) => break,
                e @ Err(Error::RxError(serial::Error::TimedOut)) => return e,
                e => {
                    if error_count < 255 {
                        error_count += 1;
                        continue
                    }
                    else {
                        return e
                    }
                }
            }
        }

        // Turn off echo on the device and wait for it to process that command
        self.send_at_command("E0")?;
        self.wait_for_ok(DEFAULT_TIMEOUT.into())?;

        Ok(())
    }

    pub fn pull_some_current(&mut self) -> return_type!(()) {
        self.chip_enable_pin.set_high().map_err(Error::PinError)?;

        self.timer.start(Millisecond(500));
        block!(self.timer.wait()).unwrap();
        self.chip_enable_pin.set_low().map_err(Error::PinError)
    }

    fn transmit_data(&mut self, data: &str) -> return_type!(()) {
        self.start_transmission(data.len())?;
        self.wait_for_prompt(DEFAULT_TIMEOUT.into())?;
        self.send_raw(data.as_bytes())?;
        self.wait_for_ok(DEFAULT_TIMEOUT.into())
    }

    fn start_tcp_connection (
        &mut self,
        connection_type: ConnectionType,
        address: &str,
        port: u16
    ) -> return_type!(())
    {
        // Length of biggest u16:
        const PORT_STRING_LENGTH: usize = 5;
        let mut port_str = ArrayString::<[_;PORT_STRING_LENGTH]>::new();
        // write!(&mut port_str, "{}", port)?;
        itoa::fmt(&mut port_str, port)?;

        self.send_raw("AT+CIPSTART=\"".as_bytes())?;
        self.send_raw(connection_type.as_str().as_bytes())?;
        self.send_raw("\",\"".as_bytes())?;
        self.send_raw(address.as_bytes())?;
        self.send_raw("\",".as_bytes())?;
        self.send_raw(port_str.as_bytes())?;
        self.send_raw("\r\n".as_bytes())?;
        self.wait_for_ok(DEFAULT_TIMEOUT.into())
    }

    fn start_transmission(&mut self, message_length: usize) -> return_type!(()) {
        // You can only send 2048 bytes per packet 
        assert!(message_length < 2048);
        let mut length_buffer = ArrayString::<[_; 4]>::new();
        // write!(&mut length_buffer, "{}", message_length)?;
        itoa::fmt(&mut length_buffer, message_length)?;

        self.send_raw(b"AT+CIPSEND=")?;
        self.send_raw(length_buffer.as_bytes())?;
        self.send_raw(b"\r\n")?;
        Ok(())
    }

    /**
      Sends the "AT${command}" to the device
    */
    fn send_at_command(&mut self, command: &str) -> return_type!(()) {
        self.send_raw(b"AT")?;
        self.send_raw(command.as_bytes())?;
        self.send_raw(b"\r\n")?;
        Ok(())
    }

    fn wait_for_at_response(
        &mut self,
        expected_response: &ATResponse,
        timeout: Millisecond
    ) -> return_type!(()) {
        let mut buffer = [0; AT_RESPONSE_BUFFER_SIZE];
        let response = serial::read_until_message(
            &mut self.rx,
            &mut self.timer,
            timeout,
            &mut buffer,
            &parse_at_response
        );

        match response {
            Ok(ref resp) if resp == expected_response => {
                Ok(())
            },
            Ok(other) => {
                Err(Error::UnexpectedResponse(other))
            }
            Err(e) => {
                Err(Error::RxError(e))
            }
        }
    }

    fn wait_for_ok(&mut self, timeout: Millisecond) -> return_type!(()) {
        self.wait_for_at_response(&ATResponse::Ok, timeout)
    }
    fn wait_for_got_ip(&mut self, timeout: Millisecond) -> return_type!(()) {
        self.wait_for_at_response(&ATResponse::WiFiGotIp, timeout)
    }

    fn wait_for_prompt(&mut self, timeout: Millisecond) -> return_type!(()) {
        let mut buffer = [0; 1];
        let result = serial::read_until_message(
            &mut self.rx,
            &mut self.timer,
            timeout,
            &mut buffer,
            &|buf, _ptr| {
                if buf[0] == '>' as u8 {
                    Some(())
                }
                else {
                    None
                }
            }
        );
        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::RxError(e))
        }
    }

    fn send_raw(&mut self, bytes: &[u8]) -> return_type!(()) {
        match serial::write_all(&mut self.tx, bytes) {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::TxError(e))
        }
    }
}

/**
  Parses `buffer` as an AT command response returning the type if it
  is a valid AT response and `None` otherwise
*/
pub fn parse_at_response(buffer: &[u8], offset: usize) -> Option<ATResponse> {
    if compare_circular_buffer(buffer, offset, "OK\r\n".as_bytes()) {
        Some(ATResponse::Ok)
    }
    else if compare_circular_buffer(buffer, offset, "ERROR\r\n".as_bytes()) {
        Some(ATResponse::Error)
    }
    else if compare_circular_buffer(buffer, offset, "busy p...\r\n".as_bytes()) {
        Some(ATResponse::Busy)
    }
    else if compare_circular_buffer(buffer, offset, "WIFI GOT IP\r\n".as_bytes()) {
        Some(ATResponse::WiFiGotIp)
    }
    else {
        None
    }
}

/**
  Compares the content of a circular buffer with another buffer. The comparison
  is done 'from the back' and if one buffer is longer than the other, only the
  content of the shared bytes is compared.

  `offset` is the index of the first byte of the circular buffer
  ```
  [4,5,0,1,2,3]
       ^- offset
  ```
*/
pub fn compare_circular_buffer(
    circular_buffer: &[u8],
    offset: usize,
    comparison: &[u8]
) -> bool
{
    let comparison_length = min(circular_buffer.len(), comparison.len());
    for i in 0..comparison_length {
        // Addition of circular_buffer.len() because % is remainder, not mathematical modulo
        // https://stackoverflow.com/questions/31210357/is-there-a-modulus-not-remainder-function-operation/31210691
        let circular_index = (circular_buffer.len() + offset - 1 - i) % circular_buffer.len();
        let comparison_index = comparison.len() - 1 - i;
        if circular_buffer[circular_index] != comparison[comparison_index] {
            return false;
        }
    }
    true
}

