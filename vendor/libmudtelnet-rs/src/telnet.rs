//! Telnet command and option constants.
//!
//! This module provides `u8` constants for Telnet opcodes and option codes
//! used across the parser and by your application code.
//!
//! Quick reference
//! ```
//! use libmudtelnet_rs::telnet::op_command as cmd;
//! use libmudtelnet_rs::telnet::op_option as opt;
//! assert_eq!(cmd::IAC, 255);
//! assert_eq!(cmd::WILL, 251);
//! assert_eq!(opt::GMCP, 201);
//! ```

// Define a public u8 constant with the given name and constant value.
macro_rules! u8_const {
  ($name: ident, $value: expr) => {
    pub const $name: u8 = $value;
  };
}

/// Module containing constants for Telnet Command codes.
// TODO(XXX): rename to cmd.
pub mod op_command {
  u8_const!(IAC, 255);
  u8_const!(WILL, 251);
  u8_const!(WONT, 252);
  u8_const!(DO, 253);
  u8_const!(DONT, 254);
  u8_const!(NOP, 241);
  u8_const!(SB, 250);
  u8_const!(SE, 240);
  u8_const!(IS, 0);
  u8_const!(SEND, 1);
  u8_const!(GA, 249);
  u8_const!(EOR, 239);
}

/// Module containing constants for Telnet Option codes.
// TODO(XXX): rename to opt.
pub mod op_option {
  u8_const!(BINARY, 0);
  u8_const!(ECHO, 1);
  u8_const!(RCP, 2);
  u8_const!(SGA, 3);
  u8_const!(NAMS, 4);
  u8_const!(STATUS, 5);
  u8_const!(TM, 6);
  u8_const!(RCTE, 7);
  u8_const!(NAOL, 8);
  u8_const!(NAOP, 9);
  u8_const!(NAOCRD, 10);
  u8_const!(NAOHTS, 11);
  u8_const!(NAOHTD, 12);
  u8_const!(NAOFFD, 13);
  u8_const!(NAOVTS, 14);
  u8_const!(NAOVTD, 15);
  u8_const!(NAOLFD, 16);
  u8_const!(XASCII, 17);
  u8_const!(LOGOUT, 18);
  u8_const!(BM, 19);
  u8_const!(DET, 20);
  u8_const!(SUPDUP, 21);
  u8_const!(SUPDUPOUTPUT, 22);
  u8_const!(SNDLOC, 23);
  u8_const!(TTYPE, 24);
  u8_const!(EOR, 25);
  u8_const!(TUID, 26);
  u8_const!(OUTMRK, 27);
  u8_const!(TTYLOC, 28);
  u8_const!(_3270REGIME, 29);
  u8_const!(X3PAD, 30);
  u8_const!(NAWS, 31);
  u8_const!(TSPEED, 32);
  u8_const!(LFLOW, 33);
  u8_const!(LINEMODE, 34);
  u8_const!(XDISPLOC, 35);
  u8_const!(ENVIRON, 36);
  u8_const!(AUTHENTICATION, 37);
  u8_const!(ENCRYPT, 38);
  u8_const!(NEWENVIRON, 39);
  u8_const!(MSSP, 70);
  u8_const!(ZMP, 93);
  u8_const!(EXOPL, 255);
  u8_const!(MCCP2, 86);
  u8_const!(MCCP3, 87);
  u8_const!(GMCP, 201);
  u8_const!(CHARSET, 42); // Character Set negotiation (RFC 2066)
  u8_const!(MSDP, 69); // MUD Server Data Protocol
  u8_const!(MXP, 91); // MUD eXtension Protocol
  u8_const!(ATCP, 200); // Achaea Telnet Client Protocol (legacy)
}

/// MSDP subnegotiation byte tags (canonical bytes only; no parser).
/// Spec: <https://tintin.mudhalla.net/protocols/msdp/>
pub mod msdp {
  u8_const!(VAR, 1); // MSDP variable tag (0x01)
  u8_const!(VAL, 2); // MSDP value tag (0x02)
  u8_const!(TABLE_OPEN, 3); // MSDP table open tag (0x03)
  u8_const!(TABLE_CLOSE, 4); // MSDP table close tag (0x04)
  u8_const!(ARRAY_OPEN, 5); // MSDP array open tag (0x05)
  u8_const!(ARRAY_CLOSE, 6); // MSDP array close tag (0x06)
}
