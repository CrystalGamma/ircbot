#![feature(io)]
#![feature(net)]
#![feature(std_misc)]
extern crate irc;
extern crate rand;
use rand::Rng;

use std::io::prelude::*;
use std::net;
use std::thread;
use std::sync::mpsc::channel;
use std::str::Pattern;
use std::sync::Arc;

struct BotConfig {
	server: net::SocketAddr,
	nick: &'static str,
	user: &'static str,
	real_name: &'static str,
	chan: irc::TargetList<'static>,
	cmd_prefix: &'static str
}

struct IrcWriter<'a>{conn: &'a mut net::TcpStream}

struct BotContext<'a> {
	conn: IrcWriter<'a>,
	nick: Arc<String>,
	logged_in: bool,
	cfg: BotConfig
}

impl<'a> IrcWriter<'a> {
	unsafe fn write_raw(&mut self, msg: String) {
		println!("< {}", msg);
		write!(self.conn, "{}\r\n",msg).unwrap();
	}
	fn write_msg(&mut self, msg: irc::TypedMessage) {
		let raw = msg.to_dumb();
		println!("< {}", raw);
		write!(self.conn, "{}\r\n", raw).unwrap();
	}
}

struct MessageContext<'a>{
	targets: irc::TargetList<'a>,
	sender: &'a str,
	nick: Arc<String>
}

impl<'a> MessageContext<'a> {
	fn reply(&self, conn: &mut IrcWriter, text: &str) {	// TODO: maybe not allocate if we have only 1 target (common case)
		let mut channels = Vec::new();
		let mut clients = Vec::new();
		for t in self.targets.iter() {
			if irc::is_channel_name(t) {
				channels.push(t);
			} else { if t != *self.nick {
				clients.push(t);
			} else {
				clients.push(self.sender)
			}}
		}
		if clients.len() > 0 {
			let target = clients.connect(",");
			conn.write_msg(irc::Notify(irc::TargetList::from_str(&target[..]), text));
		}
		if channels.len() > 0 {
			let target = channels.connect(",");
			conn.write_msg(irc::Talk(irc::TargetList::from_str(&target[..]), text));
		}
	}

	fn new(sender: &'a str, targets: irc::TargetList<'a>, ctx: &BotContext) -> MessageContext<'a> {
		MessageContext {targets: targets, sender: irc::nick_from_mask(sender), nick: ctx.nick.clone()}
	}
}

fn handle_cmd(cmd: &str, msg_ctx: MessageContext, ctx: &mut BotContext) -> bool {
	let cmd2 = cmd.trim_left();
	let (verb, args) = cmd2.find(|c:char| c.is_whitespace()).map_or_else(|| (cmd2.trim_right(), ""),
								|p| (&cmd2[..p], cmd2[(p+1)..].trim()));
	match verb {
		"say" => ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, args)),
		"roll" => {
			let (num_dice, num_sides) = args.find('d').map_or_else(|| (args, "6"), |p| (&args[..p], &args[(p+1)..]));
			match (num_dice.parse::<u16>(), num_sides.parse::<u16>()) {
				(Ok(dice), Ok(sides)) => {
					let mut rng = rand::thread_rng();
					let res = range(0, dice).map(|_| rng.gen_range(0, sides) as u32+1).fold(0, |a, b| a+b);
					let reply = format!("{}d{}: {}", dice, sides, res);
					msg_ctx.reply(&mut ctx.conn, &reply[..])
				}
				_ => msg_ctx.reply(&mut ctx.conn, "usage (dice, sides < 65536): roll <dice>[d<sides>]")
			}
		},
		_ => {}
	}
	true
}

fn handle_msg(msg: irc::IrcMessage, ctx: &mut BotContext) -> bool {
	use irc::*;
	let semantic = analyse_message(msg);
	match semantic {
		Msg(sender, targets, text) => if ctx.cfg.cmd_prefix.is_prefix_of(text) {
			let cmd = &text[ctx.cfg.cmd_prefix.len()..];
			let msg_ctx = MessageContext::new(sender, targets, ctx);
			handle_cmd(cmd, msg_ctx, ctx)
		} else {true},
		Welcome(_) => {
			ctx.logged_in = true;
			ctx.conn.write_msg(Join(ctx.cfg.chan, None));
			true
		},
		Joined(client, list) => {
			if irc::nick_from_mask(client) == ctx.cfg.nick {
				for chan in list.iter() {
					let msg = format!("Hello, {channel}!", channel=chan);
					ctx.conn.write_msg(Talk(TargetList::from_str(chan), "Whatever you $say, my child ..."));
				}
			}
			true
		},
		Ping(packets) => {ctx.conn.write_msg(Pong(packets)); true},
		Topic(..) => true,
		_ if !semantic.is_motd() => {println!("{:?}", semantic); true},
		_ => true
	}
}

fn bot(cfg: BotConfig) {
    let mut conn = net::TcpStream::connect(&cfg.server).unwrap();
	let conn_read = conn.try_clone().ok().unwrap();
	let (tx, rx) = channel();
	let irc_recv = thread::scoped(move ||{
		for s in irc::IrcReader::new(std::io::BufReader::new(conn_read)) {
			match s {
				Ok(m) => {
					println!("> {}", m);
					tx.send(Ok(m)).unwrap();
				}, Err(e) => {
					tx.send(Err(e)).ok().unwrap();
					return;
				}
			};
		}
	});
	let mut write = IrcWriter{conn: &mut conn};
	write.write_msg(irc::SetNick(cfg.nick));
	write.write_msg(irc::Register(cfg.user, cfg.real_name));
	let mut ctx = BotContext {conn: write, nick: Arc::new(cfg.nick.to_string()), logged_in: false, cfg: cfg};
	loop {
		select! {
			m = rx.recv() => match m.unwrap() { Ok(s) => {
				let msg = match irc::parse_irc_message(&s[..]) {
					Some(x) => x,
					None => continue
				};
				if !handle_msg(msg, &mut ctx) {
					break;
				}
			}, Err(e) => {
				println!("Recv error: {}", e);
				break;
			}}
		}
	}
	irc_recv.join();
	println!("Disconnected");
}

fn main() {
	bot(BotConfig {
		server: net::ToSocketAddrs::to_socket_addrs(&("irc.quakenet.org", 6667)).unwrap().next().expect("could not look up server hostname"),
		nick: "RidikaLukria",
		user: "ridika",
		real_name: "(I'm a bot) Ridika Lūkria, Göttin des Feuers",
		chan: irc::TargetList::from_str("#Deathmictest"),
		cmd_prefix: "$"
	})
}
