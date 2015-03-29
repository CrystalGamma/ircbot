#![feature(std_misc)]
#![feature(core)]
extern crate irc;
extern crate rand;
use rand::Rng;
extern crate hyper;
extern crate rustc_serialize as serialize;
use serialize::json::{Json, ToJson};
extern crate chrono;
use chrono::offset::TimeZone;
extern crate regex;
use regex::Regex;

use std::io::prelude::*;
use std::net;
use std::thread;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::str::Pattern;
use std::sync::Arc;
use std::collections::HashMap;
use std::path::Path;

macro_rules! tryopt{
	($e:expr) => (match $e {Some(x) => x, _ => return None})
}

mod joindb;
use joindb::*;

fn exchange<T>(a: &mut T, mut b: T) -> T {
	std::mem::swap(a, &mut b);
	b
}

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
	cfg: BotConfig,
	poll: Option<(String, Vec<(String, u32)>)>,
	stream_requests: Sender<StreamListenerRequest>,
	stream_resources: Option<(Sender<(Option<String>, String)>, Receiver<StreamListenerRequest>)>,
	title_tx: Sender<String>,
	joins_tx: Sender<String>
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

	fn reply_private(&self, conn: &mut IrcWriter, text: &str) {
		conn.write_msg(irc::Notify(irc::TargetList::from_str(self.sender), text));
	}

	fn get_sender_nick(&self) -> &'a str {
		self.sender
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
		"poll" => {
			if ctx.poll != None {
				let error = format!("A poll is already running. End it with {}endpoll", ctx.cfg.cmd_prefix);
				msg_ctx.reply(&mut ctx.conn, &error[..]);
				return true;
			}
			let mut parts = args.split('|').map(|s| s.trim());
			let question = match parts.next() {
				None | Some("") => {msg_ctx.reply(&mut ctx.conn, "usage: poll question|answer1|answer2|answer3 ..."); return true},
				Some(s) => s.to_string()
			};
			let mut answers: Vec<_> = parts.map(|a| (a.to_string(), 0)).collect();
			if answers.len() == 0 {
				answers.push(("me".to_string(), 0));
			}
			let qmsg = format!("{} started a poll for: {}", msg_ctx.get_sender_nick(), question);
			ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &qmsg[..]));
			for (count, &(ref ans, _)) in answers.iter().enumerate() {
				let amsg = format!("/msg {} {}vote {} for: {}", ctx.nick, ctx.cfg.cmd_prefix, count + 1, &ans[..]);
				ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &amsg[..]));
			}
			ctx.poll = Some((question, answers));
		},
		"vote" => {
			match ctx.poll {None => {
				msg_ctx.reply(&mut ctx.conn, "no poll is running");
			}, Some((_, ref mut answers)) => {
				let len = answers.len();
				let id = if args == "" {1} else { match args.trim().parse() {Ok(x) => x, Err(_) => {
					msg_ctx.reply_private(&mut ctx.conn, &format!("argument must be an integer in range 1-{}", len));
					return true;
				}}};
				match answers.get_mut(id - 1) {None => {
					let error = format!("there are only {} answers, can't vote for no. {}", len, id);
					msg_ctx.reply_private(&mut ctx.conn, &error[..]);
				}, Some(&mut (ref ans, ref mut votes)) => {
					*votes += 1;
					let msg = format!("cast vote for: {}", &ans[..]);
					msg_ctx.reply_private(&mut ctx.conn, &msg[..]);
				}}
			}}
		},
		"endpoll" => {
			match exchange(&mut ctx.poll, None) {None => {
				msg_ctx.reply(&mut ctx.conn, "no poll is running");
			}, Some((question, answers)) => {
				let total_votes = answers.iter().map(|&(_, votes)| votes).fold(0, |a,b|a+b);
				if total_votes == 0 {
					ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &format!("no votes were cast regarding: {}", question)));
					return true;
				}
				ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &format!("Poll results for: {}", question)));
				if answers.len() > 1 {for (answer, votes) in answers {
					ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &format!("{}/{} = {}% of votes for: {}", votes,
					total_votes, 100.0*(votes as f32)/(total_votes as f32), answer)));
				}} else {
					ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &format!("{} voted: {}", total_votes, answers[0].0)));
				}
			}}
		},
		"streams" => ctx.stream_requests.send(ListAllStreams(Some(msg_ctx.get_sender_nick().to_string()))).unwrap(),
		"addstream" | "rmstream" | "removestream" => match (verb, args.find(' ')
				.and_then(|p| Stream::new(&args[..p], args[p..].trim_left().to_string()))) {
			("addstream", Some(stream)) => ctx.stream_requests.send(AddStream(stream)).unwrap(),
			("rmstream", Some(stream))|("removestream", Some(stream)) => ctx.stream_requests.send(RemoveStream(stream)).unwrap(),
			_ => msg_ctx.reply(&mut ctx.conn, &format!("usage (provider is one of 'twitch', 'hitbox'): {} <provider> <name>", verb))
		},
		"say" => ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, args)),
		"roll" => {
			let (num_dice, num_sides) = args.find('d').map_or_else(|| (args, "6"), |p| (&args[..p], &args[(p+1)..]));
			match (num_dice.parse::<u16>(), num_sides.parse::<u16>()) {
				(Ok(dice), Ok(sides)) => {
					let mut rng = rand::thread_rng();
					let res = (0..dice).map(|_| rng.gen_range(0, sides) as u32+1).fold(0, |a, b| a+b);
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

#[derive(PartialEq,Eq,Hash,Clone,Debug)]
enum Stream {
	Hitbox(String),
	Twitch(String)
}
use Stream::*;

#[derive(PartialEq)]
struct StreamStatus(Option<(String, Option<String>, chrono::DateTime<chrono::Local>)>);

impl StreamStatus {
	fn is_online(&self) -> bool {match self {
		&StreamStatus(None) => false,
		_ => true
	}}

	fn link(&self) -> Option<&str> {match self.0 {
		Some((ref s, _, _)) => Some(&s[..]),
		_ => None
	}}
	fn game(&self) -> Option<&str> {match self.0 {
		Some((_, Some(ref s), _)) => Some(&s[..]),
		_ => None
	}}
	fn start_time(&self) -> Option<chrono::DateTime<chrono::Local>> { match self.0{
		Some((_, _, ref time)) => Some(time.clone()),
		_ => None
	}}
}

impl Stream {
	fn get_status<C: hyper::net::NetworkConnector>(&self, http: &mut hyper::Client<C>) -> Result<StreamStatus, String> {
		match self {
			&Twitch(ref name) => http.get(&format!("https://api.twitch.tv/kraken/streams/{}", name)[..])
				.header(hyper::header::Accept(
					vec![hyper::header::qitem("application/vnd.twitchtv.v3+json".parse().ok().expect("couldnt parse MIME type"))]
				)).send().map_err(|_|"could not query api.twitch.tv".to_string())
				.and_then(|mut response| {
					if response.status != hyper::Ok {return Err(format!("{}", response.status))}
					let mut buf = String::new();
					response.read_to_string(&mut buf).map_err(|e|format!("{}", e)).map(|_|buf)
				}).and_then(|s| Json::from_str(&s[..]).map_err(|e| format!("{}", e)))
				.and_then(|json|{
					json.as_object().ok_or_else(||"Twitch API didn't return a JSON object".to_string())
					.and_then(|obj| obj.get("stream").ok_or_else(
						||"'stream' property missing in JSON document".to_string()))
						.and_then(|json| match json.as_object() {
							Some(obj) => obj.get("created_at").ok_or_else(||"'created_at' property missing from stream status".to_string())
								.and_then(|json| json.as_string().ok_or_else(||"'created_at' property of stream status is not a string".to_string()))
								.and_then(|s| s.parse().map_err(|_|"could not parse stream start time".to_string())
								.map(|x|StreamStatus(Some((format!("http://twitch.tv/{}", name), None, x))))),
							None => Ok(StreamStatus(None))
						})
				}),
			&Hitbox(ref name) => http.get(&format!("http://api.hitbox.tv/media/live/{}", name)[..]).send()
				.map_err(|e|format!("HTTP send error: {}", e))
				.and_then(|mut response|{
					if response.status != hyper::Ok {return Err(format!("{}", response.status))}
					let mut buf = String::new();
					response.read_to_string(&mut buf).map_err(|e|format!("HTTP error: {}", e)).map(|_|buf)
				}).and_then(|s|Json::from_str(&s[..]).map_err(|e|format!("JSON parser: {}",e)))
				.and_then(
					|json: Json|json.as_object().ok_or("Hitbox API didn't return a JSON object".to_string())
					.and_then(|obj| obj.get("livestream").ok_or_else(||"'livestream' property missing in JSON document".to_string()))
					.and_then(|json|json.as_array().ok_or_else(||"'livestream' property is not an array".to_string()))
					.and_then(|arr| arr.iter().fold(Ok(StreamStatus(None)),
						|res, json: &Json| res.and_then(|online| if online.is_online() {
							json.as_object().ok_or_else(||"one of the 'livestream' entries is not a JSON object".to_string())
							.and_then(|obj|match obj.get("media_is_live") {
								Some(json) => if match json.as_string() {
									None => return Err("'media_is_live' is not a string".to_string()),
									Some(x) => x
								} == "0" { Ok(StreamStatus(None)) } else {
									match obj.get("media_live_since").and_then(|json|json.as_string()) {
										None => Err("'media_live_since' is not a string".to_string()),
										Some(start) => chrono::NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M:%S")
											.map(|naive|chrono::Local.from_utc_datetime(&naive))
											.map(|x|StreamStatus(Some((format!("http://hitbox.tv/{}", name), None, x))))
											.map_err(|e| format!("Time parsing error: {}", e))
									}

								},
								None => Err("'media_is_live' is missing from one of the stream objects".to_string())
							})
						} else {Ok(online)}
					))))
		}
	}
	fn link(&self) -> String {
		match self {
			&Hitbox(ref name) => format!("http://hitbox.tv/{}", name),
			&Twitch(ref name) => format!("http://twitch.tv/{}", name)
		}
	}
	fn path(&self) -> std::path::PathBuf {
		std::path::Path::new(match self {
			&Hitbox(_) => "hitbox",
			&Twitch(_) => "twitch"
		}).join(match self{&Twitch(ref x)|&Hitbox(ref x)=>x})
	}
	fn create_entry(&self) -> std::io::Result<()> {
		std::fs::File::create(&self.path()).map(|_| ())
	}
	fn remove_entry(&self) -> std::io::Result<()> {
		std::fs::remove_file(&self.path())
	}
	fn new(provider: &str, name: String) -> Option<Stream> {
		if name.contains('/') { return None }	// FIXME: maybe find a crossplatforn way to prevent filesystem access?
		match provider {
			"hitbox" | "hitbox.tv" => Some(Hitbox(name)),
			"twitch" | "twitch.tv" => Some(Twitch(name)),
			_ => None
		}
	}
}

fn stream_map(dir: &str) -> Vec<String> {
	match std::fs::read_dir(dir) {
		Ok(rd) => rd.filter_map(|de| match de {
			Ok(de) => de.path().file_name().and_then(|s| s.to_str()).map(|s| s.to_string()),
			Err(_) => None
		}).collect(),
		Err(_) => {std::fs::create_dir(dir).unwrap(); Vec::new()}
	}
}

enum StreamListenerRequest {
	ListOnlineStreams(String),
	ListAllStreams(Option<String>),
	UpdateStreams,
	AddStream(Stream),
	RemoveStream(Stream)
}
use StreamListenerRequest::*;

fn on_joined(chan_list: irc::TargetList, ctx: &mut BotContext) {
	use irc::*;
	for chan in chan_list.iter() {
		let msg = format!("Hello, {channel}!", channel=chan);
		ctx.conn.write_msg(Talk(TargetList::from_str(chan), &msg[..]));
	}
	let pulses = ctx.stream_requests.clone();
	match exchange(&mut ctx.stream_resources, None) {None => {},Some((stream_events_tx, stream_requests_rx)) => {thread::spawn(move || {
		let mut streams: HashMap<_,_> = stream_map("twitch").into_iter().map(|name| (Twitch(name), Err("not checked yet".to_string())))
		.chain(stream_map("hitbox").into_iter().map(|name|(Hitbox(name), Err("not checked yet".to_string())))).collect();
		let pulse_thread = std::thread::scoped(move ||{
			pulses.send(UpdateStreams).unwrap();
			loop {
				std::thread::park_timeout(std::time::Duration::seconds(10));
				pulses.send(UpdateStreams).unwrap();
			}
		});
		let mut client = hyper::Client::new();
		loop {match stream_requests_rx.recv().unwrap() {UpdateStreams => {
			println!("pulse");
			for (stream, status) in streams.iter_mut() {
				let new_status = stream.get_status(&mut client);
				if new_status != *status {
					match new_status {
						Ok(ref online) => stream_events_tx.send((None, if online.is_online() {match online.game() {
							None => format!("=== {:?} has gone on air! === {}", stream, stream.link()),
							Some(game) => format!("=== {:?} has gone on air! === Game: {} === {}", stream, game, online.link().unwrap()),
						}} else {
							format!("{:?} has gone offline", stream)
						})).unwrap(),
						Err(ref msg) => stream_events_tx.send((None, format!("Could not check status of Stream {:?}: {}", stream, msg.clone()))).unwrap()
					}
					*status = new_status;
				}
			}
		},
		ListOnlineStreams(target) => {
			for (stream, status) in streams.iter() {match *status {
				Ok(ref status) if status.is_online() => stream_events_tx.send((Some(target.clone()),
					format!("Stream {:?} is streaming since {}!", stream, status.start_time().unwrap()))).unwrap(),
			_=>{}}}
		}
		ListAllStreams(target) => {
			for (stream, status) in streams.iter() {stream_events_tx.send((target.clone(), match *status {
				Ok(ref status)  => if status.is_online() {
					format!("Stream {:?} is streaming since {}!", stream, status.start_time().unwrap())
				} else {
					format!("Stream {:?} is offline", stream)
				},
				Err(ref e) => format!("status of Stream {:?} could not be checked: {}", stream, e),
			})).unwrap()}
		},
		AddStream(stream) => if !streams.contains_key(&stream) {
			match stream.create_entry() {
				Ok(_) => stream_events_tx.send((None, format!("{:?} added to the stream list", stream))).unwrap(),
				Err(e) =>  stream_events_tx.send((None, format!("could not add {:?} to the stream list: {}", stream, e))).unwrap()
			}
			streams.insert(stream, Err("not checked yet".to_string()));
		} else {stream_events_tx.send((None, format!("{:?} is already on the stream list", stream))).unwrap()},
		RemoveStream(stream) => if streams.contains_key(&stream) {
			match stream.remove_entry() {
				Ok(_) => stream_events_tx.send((None, format!("{:?} removed from the stream list", stream))).unwrap(),
				Err(e) =>  stream_events_tx.send((None, format!("could not remove {:?} from the stream list: {}", stream, e))).unwrap()
			}
			streams.remove(&stream);
		} else {stream_events_tx.send((None, format!("{:?} is not on the stream list", stream))).unwrap()}}}
	});}};
}

fn handle_msg(msg: irc::IrcMessage, ctx: &mut BotContext) -> bool {
	use irc::*;
	let semantic = analyse_message(msg);
	match semantic {
		Msg(sender, targets, text) => if ctx.cfg.cmd_prefix.is_prefix_of(text) {
			let cmd = &text[ctx.cfg.cmd_prefix.len()..];
			let msg_ctx = MessageContext::new(sender, targets, ctx);
			handle_cmd(cmd, msg_ctx, ctx)
		} else {
			for word in text.split(|c:char| c.is_whitespace()) {
				if word.len() < 4 || word.ends_with('.') || !word.contains(|c| c=='.') {continue}
				let tx = ctx.title_tx.clone();
				let url = if word.starts_with("http://") || word.starts_with("https://") {
					word.to_string()
				} else {
					format!("http://{}",word)
				};
				// TODO: Imgur handling
				thread::spawn(move ||{
					let mut http = hyper::Client::new();
					match http.get(&url[..]).send()
						.map_err(|e|format!("HTTP send error: {}", e))
						.and_then(|mut response|{
							if response.status != hyper::Ok {return Err(format!("{}", response.status))}
							let mut buf = String::new();
							response.read_to_string(&mut buf).map_err(|e|format!("HTTP error: {}", e)).map(|_|buf)
						}).and_then(|text| Regex::new("(?i)<\\s*title\\s*>([^<]*)<\\s*/title\\s*>").unwrap()	// yes, shame on me for using a regex for this
							.captures(&text).ok_or_else(||"no title tag".to_string())
							.and_then(|cap| cap.at(1).ok_or_else(|| "no match".to_string()))
							.map(|title| title.to_string())) {
						Ok(title) => tx.send(title).unwrap(),
						Err(msg) => {}
					}
				});
			}
			true
		},
		Welcome(_) => {
			ctx.logged_in = true;
			ctx.conn.write_msg(Join(ctx.cfg.chan, None));
			true
		},
		Joined(client, list) => {
			let nick = irc::nick_from_mask(client);
			if nick == ctx.cfg.nick {
				on_joined(list, ctx);
			} else {
				ctx.stream_requests.send(ListOnlineStreams(nick.to_string())).unwrap();
				ctx.joins_tx.send(nick.to_string()).unwrap();
			}
			true
		},
		Ping(packets) => {ctx.conn.write_msg(Pong(packets)); true},
		Topic(..) => true,
		_ if !semantic.is_motd() => {println!("{:?}", semantic); true},
		_ => true
	}
}

fn ordinal(i: u64) -> &'static str {
	match i % 10 {
		1 if i != 11 => "st",
		2 if i != 12 => "nd",
		3 if i != 13 => "rd",
		_ => "th"
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
	let (stream_events_tx, stream_events) = channel();
	let (stream_requests, stream_requests_rx) = channel();
	let (title_tx, title_rx) = channel();
	let (joins_tx, joins_rx) = channel();
	let (joinlog_tx, joinlog_rx) = channel();
	let mut ctx = BotContext {
		conn: write,
		nick: Arc::new(cfg.nick.to_string()),
		logged_in: false,
		cfg: cfg,
		poll: None,
		stream_requests: stream_requests,
		stream_resources: Some((stream_events_tx, stream_requests_rx)),
		title_tx: title_tx,
		joins_tx: joins_tx
	};
	thread::spawn(move ||{
		let mut joins = JsonJoinDB::new("joinlog".to_string());
		while let Ok(nick) = joins_rx.recv() {
			match joins.join(&nick) {
				(nick, Some(user)) => {
					let count = user.num_visits + 1;
					joinlog_tx.send(format!("Welcome back my child {}. This is your {}{} visit that I have witnessed.",
						nick, count, ordinal(count))).unwrap()
				},
				(nick, None) => joinlog_tx.send(format!("Hello {}. I don't think I have seen you around. Make yourself at home.", nick)).unwrap()
			}
		}
	});
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
			}},
			m = stream_events.recv() => match m.unwrap() {
				(None, msg) => ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &msg[..])),
				(Some(target), msg) => ctx.conn.write_msg(irc::Notify(irc::TargetList::from_str(&target[..]), &msg[..]))
			},
			m = title_rx.recv() => ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &m.unwrap()[..])),
			m = joinlog_rx.recv() => ctx.conn.write_msg(irc::Talk(ctx.cfg.chan, &m.unwrap()[..]))
		}
	}
	irc_recv.join();
	println!("Disconnected");
}

fn main() {
	bot(BotConfig {
		server: net::ToSocketAddrs::to_socket_addrs(&("irc.quakenet.org", 6667)).unwrap().next().expect("server hostname not found"),
		nick: "RidikaLukria",
		user: "ridika",
		real_name: "(I'm a bot) Ridika Lūkria, Göttin des Feuers",
		chan: irc::TargetList::from_str("#Deathmictest"),
		cmd_prefix: "$"
	})
}
