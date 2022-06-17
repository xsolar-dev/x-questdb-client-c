/*******************************************************************************
 *     ___                  _   ____  ____
 *    / _ \ _   _  ___  ___| |_|  _ \| __ )
 *   | | | | | | |/ _ \/ __| __| | | |  _ \
 *   | |_| | |_| |  __/\__ \ |_| |_| | |_) |
 *    \__\_\\__,_|\___||___/\__|____/|____/
 *
 *  Copyright (c) 2014-2019 Appsicle
 *  Copyright (c) 2019-2022 QuestDB
 *
 *  Licensed under the Apache License, Version 2.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  http://www.apache.org/licenses/LICENSE-2.0
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 *
 ******************************************************************************/

use std::convert::{TryFrom, TryInto, Infallible};
use std::fmt;
use std::fmt::{Write, Display, Formatter};
use std::io;
use std::io::Write as IoWrite;
use socket2::{Domain, Socket, SockAddr, Type, Protocol};

#[derive(Debug, Copy, Clone)]
enum Op {
    Table = 1,
    Symbol = 1 << 1,
    Column = 1 << 2,
    At = 1 << 3,
    Flush = 1 << 4
}

impl Op {
    fn descr(self) -> &'static str {
        match self {
            Op::Table => "table",
            Op::Symbol => "symbol",
            Op::Column => "column",
            Op::At => "at",
            Op::Flush => "flush"
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum State {
    Connected =
        Op::Table as isize,
    TableWritten =
        Op::Symbol as isize | Op::Column as isize,
    SymbolWritten =
        Op::Symbol as isize | Op::Column as isize | Op::At as isize,
    ColumnWritten =
        Op::Column as isize | Op::At as isize,
    MayFlushOrTable =
        Op::Flush as isize | Op::Table as isize,
    Moribund = 0,
}

impl State {
    fn next_op_descr(self) -> &'static str {
        match self {
            State::Connected => "should have called `table` instead",
            State::TableWritten => "should have called `symbol` or `column` instead",
            State::SymbolWritten => "should have called `symbol`, `column` or `at` instead",
            State::ColumnWritten => "should have called `column` or `at` instead",
            State::MayFlushOrTable => "should have called `flush` or `table` instead",
            State::Moribund => "unrecoverable state due to previous error"
        }
    }
}

/// Category of error.
#[derive(Debug, Copy, Clone)]
pub enum ErrorCode {
    /// The host, port, or interface was incorrect.
    CouldNotResolveAddr,

    /// Called methods in the wrong order. E.g. `symbol` after `column`.
    InvalidApiCall,

    /// A network error connecting of flushing data out.
    SocketError,

    /// The string or symbol field is not encoded in valid UTF-8.
    InvalidUtf8,

    /// The table name, symbol name or column name contains bad characters.
    InvalidName
}

#[derive(Debug)]
pub struct Error {
    code: ErrorCode,
    msg: String
}

impl Error {
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn msg(&self) -> &str {
        &self.msg
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn map_io_to_socket_err(prefix: &str, io_err: io::Error) -> Error {
    Error {
        code: ErrorCode::SocketError,
        msg: format!("{}{}", prefix, io_err)
    }
}

pub struct Name<'a> {
    name: &'a str
}

impl <'a> Name<'a> {
    pub fn new(name: &'a str) -> Result<Self> {
        if name.is_empty() {
            return Err(Error{
                code: ErrorCode::InvalidName,
                msg: "table, symbol and column names must have a non-zero length.".to_owned()});
        }

        for (index, c) in name.chars().enumerate() {
            match c {
                ' ' | '?' | '.' | ',' | '\'' | '\"' | '\\' | '/' | '\0' |
                ':' | ')' | '(' | '+' | '-' | '*' | '%' | '~' => {
                    return Err(Error{
                        code: ErrorCode::InvalidName,
                        msg: format!(
                            concat!(
                                "Bad string {:?}: ",
                                "table, symbol and column names can't contain a {:?} ",
                                "character, which was found at byte position {}."),
                            name,
                            c,
                            index)});
                },
                '\u{FEFF}' => {
                    // Reject unicode char 'ZERO WIDTH NO-BREAK SPACE', aka UTF-8 BOM
                    // if it appears anywhere in the string.
                    return Err(Error{
                        code: ErrorCode::InvalidName,
                        msg: format!(
                            concat!(
                                "Bad string {:?}: ",
                                "table, symbol and column names can't contain a UTF-8 BOM ",
                                "character, which was found at byte position {}."),
                            name,
                            index)});
                },
                _ => ()
            }
        }

        Ok(Self { name: name })
    }
}

impl <'a> TryFrom<&'a str> for Name<'a> {
    type Error = self::Error;

    fn try_from(name: &'a str) -> Result<Self> {
        Name::new(name)
    }
}

impl From<Infallible> for Error {
    fn from(_: Infallible) -> Self {
        unreachable!()
    }
}

fn write_escaped_impl<Q, C>(check_escape_fn: C, quoting_fn: Q, output: &mut String, s: &str)
    where
        C: Fn(char) -> bool,
        Q: Fn(&mut String) -> () {
    let mut to_escape = 0usize;
    for c in s.chars() {
        if check_escape_fn(c) {
            to_escape += 1;
        }
    }

    quoting_fn(output);

    if to_escape == 0 {
        output.push_str(s);
    }
    else {
        output.reserve(s.len() + to_escape);
        for c in s.chars() {
            if check_escape_fn(c) {
                output.push('\\');
            }
            output.push(c);
        }
    }

    quoting_fn(output);
}

fn must_escape_unquoted(c: char) -> bool {
    match c {
        ' ' | ',' | '=' | '\n' | '\r' | '"' | '\\' => true,
        _ => false
    }
}

fn must_escape_quoted(c: char) -> bool {
    match c {
        '\n' | '\r' | '"' | '\\' => true,
        _ => false
    }
}

fn write_escaped_unquoted(output: &mut String, s: &str) {
    write_escaped_impl(
        must_escape_unquoted,
        |_output| (),
        output,
        s);
}

fn write_escaped_quoted(output: &mut String, s: &str) {
    write_escaped_impl(
        must_escape_quoted,
        |output| output.push('"'),
        output,
        s)
}

pub struct LineSender {
    sock: Socket,
    state: State,
    output: String,
    last_line_start: usize
}

impl LineSender {
    pub fn connect(host: &str, port: &str, net_interface: Option<&str>) -> Result<Self> {
        let addr: SockAddr = gai::resolve_host_port(host, port)?;
        let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .map_err(|io_err| map_io_to_socket_err("Could not open TCP socket: ", io_err))?;
        sock.set_nodelay(true)
            .map_err(|io_err| map_io_to_socket_err("Could not set TCP_NODELAY: ", io_err))?;
        if let Some(host) = net_interface {
            let bind_addr = gai::resolve_host(host)?;
            sock.bind(&bind_addr)
            .map_err(|io_err| map_io_to_socket_err(
                &format!("Could not bind to interface address {:?}: ", host), io_err))?;
        }
        sock.connect(&addr)
            .map_err(|io_err| {
                let host_port = format!("{}:{}", host, port);
                let prefix = format!("Could not connect to {:?}: ", host_port);
                map_io_to_socket_err(&prefix, io_err)
            })?;
        Ok(Self {
            sock: sock,
            state: State::Connected,
            output: String::with_capacity(65536),
            last_line_start: 0usize
        })
    }

    fn check_state(&mut self, op: Op) -> Result<()> {
        if (self.state as isize & op as isize) > 0 {
            return Ok(());
        }
        let error = Error{
            code: ErrorCode::InvalidApiCall,
            msg: format!(
                "State error: Bad call to `{}`, {}. Must now call `close`.",
                op.descr(),
                self.state.next_op_descr())};
        self.state = State::Moribund;
        Err(error)
    }

    pub fn table<'a, N, E>(&mut self, name: N) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        let name: Name<'a> = name.try_into()?;
        self.check_state(Op::Table)?;
        write_escaped_unquoted(&mut self.output, name.name);
        self.state = State::TableWritten;
        Ok(())
    }

    pub fn symbol<'a, N, E>(&mut self, name: N, value: &str) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        let name: Name<'a> = name.try_into()?;
        self.check_state(Op::Symbol)?;
        self.output.push(',');
        write_escaped_unquoted(&mut self.output, name.name);
        self.output.push('=');
        write_escaped_unquoted(&mut self.output, value);
        self.state = State::SymbolWritten;
        Ok(())
    }

    fn write_column_key<'a, N, E>(&mut self, name: N) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        let name: Name<'a> = name.try_into()?;
        self.check_state(Op::Column)?;
        self.output.push(
            if (self.state as isize & Op::Symbol as isize) > 0 {
                ' '
            } else {
                ','
            });
        write_escaped_unquoted(&mut self.output, name.name);
        self.output.push('=');
        self.state = State::ColumnWritten;
        Ok(())
    }

    pub fn column_bool<'a, N, E>(&mut self, name: N, value: bool) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        self.write_column_key(name)?;
        self.output.push(if value {'t'} else {'f'});
        Ok(())
    }

    pub fn column_i64<'a, N, E>(&mut self, name: N, value: i64) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        self.write_column_key(name)?;
        write!(&mut self.output, "{}i", value).unwrap();
        Ok(())
    }

    pub fn column_f64<'a, N, E>(&mut self, name: N, value: f64) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        self.write_column_key(name)?;
        if value == f64::INFINITY {
            self.output.push_str("Infinity");
        }
        else if value == f64::NEG_INFINITY {
            self.output.push_str("-Infinity");
        }
        else {
            write!(&mut self.output, "{}", value).unwrap();
        }
        Ok(())
    }

    pub fn column_str<'a, N, E>(&mut self, name: N, value: &str) -> Result<()>
        where
            N: TryInto<Name<'a>, Error=E>,
            Error: From<E>
    {
        self.write_column_key(name)?;
        write_escaped_quoted(&mut self.output, value);
        Ok(())
    }

    pub fn pending_size(&self) -> usize {
        if self.state != State::Moribund {
            self.output.len()
        }
        else {
            0
        }
    }

    fn update_last_line_start(&mut self) {
        self.last_line_start = self.pending_size();
    }

    pub fn at(&mut self, epoch_nanos: i64) -> Result<()> {
        self.check_state(Op::At)?;
        write!(&mut self.output, " {}\n", epoch_nanos).unwrap();
        self.update_last_line_start();
        self.state = State::MayFlushOrTable;
        Ok(())
    }

    pub fn at_now(&mut self) -> Result<()> {
        self.check_state(Op::At)?;
        self.output.push('\n');
        self.update_last_line_start();
        self.state = State::MayFlushOrTable;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.check_state(Op::Flush)?;
        let buf = self.output.as_bytes();
        if let Err(io_err) = self.sock.write_all(buf) {
            self.state = State::Moribund;
            return Err(map_io_to_socket_err(
                "Could not flush buffered messages: ",
                io_err));
        }
        self.output.clear();
        self.state = State::Connected;
        Ok(())
    }

    pub fn must_close(&self) -> bool {
        self.state == State::Moribund
    }
}

mod gai;

#[allow(non_camel_case_types)]
#[cfg(feature = "ffi")]
pub mod ffi;