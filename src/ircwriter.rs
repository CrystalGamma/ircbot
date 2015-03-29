/* an IRC bot
    Copyright (C) 2015 Jona Stubbe

    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.*/
use irc::*;
use std::net::TcpStream;
use std::sync::Arc;
use std::io::prelude::*;

pub struct IrcWriter<'a>(&'a mut TcpStream);

impl<'a> IrcWriter<'a> {
	pub fn new(conn: &mut TcpStream) -> IrcWriter {IrcWriter(conn)}
	pub unsafe fn write_raw(&mut self, msg: String) {
		println!("< {}", msg);
		write!(self.0, "{}\r\n",msg).unwrap();
	}
	pub fn write_msg(&mut self, msg: TypedMessage) {
		let raw = msg.to_dumb();
		println!("< {}", raw);
		write!(self.0, "{}\r\n", raw).unwrap();
	}
}

pub struct MessageContext<'a>{
	targets: TargetList<'a>,
	sender: &'a str,
	nick: Arc<String>
}

impl<'a> MessageContext<'a> {
	pub fn reply(&self, conn: &mut IrcWriter, text: &str) {	// TODO: maybe not allocate if we have only 1 target (common case)
		let mut channels = Vec::new();
		let mut clients = Vec::new();
		for t in self.targets.iter() {
			if is_channel_name(t) {
				channels.push(t);
			} else { if t != *self.nick {
				clients.push(t);
			} else {
				clients.push(self.sender)
			}}
		}
		if clients.len() > 0 {
			let target = clients.connect(",");
			conn.write_msg(Notify(TargetList::from_str(&target[..]), text));
		}
		if channels.len() > 0 {
			let target = channels.connect(",");
			conn.write_msg(Talk(TargetList::from_str(&target[..]), text));
		}
	}

	pub fn reply_private(&self, conn: &mut IrcWriter, text: &str) {
		conn.write_msg(Notify(TargetList::from_str(self.sender), text));
	}

	pub fn get_sender_nick(&self) -> &'a str {
		self.sender
	}

	pub fn new(sender: &'a str, targets: TargetList<'a>, nick: &Arc<String>) -> MessageContext<'a> {
		MessageContext {targets: targets, sender: nick_from_mask(sender), nick: nick.clone()}
	}
}