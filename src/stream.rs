use hyper::{Client, header};
use hyper::Ok as HttpOk;
use chrono::{Local, DateTime, NaiveDateTime};
use chrono::offset::TimeZone;
use serialize::json::Json;
use std::path::{Path, PathBuf};
use std::io::Result as IoResult;
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
use std::io::prelude::*;
use std::fs;

#[derive(PartialEq,Eq,Hash,Clone,Debug)]
pub enum Stream {
	Hitbox(String),
	Twitch(String)
}
pub use self::Stream::*;

#[derive(PartialEq)]
pub struct StreamStatus(Option<(String, Option<String>, DateTime<Local>)>);

impl StreamStatus {
	pub fn is_online(&self) -> bool {match self {
		&StreamStatus(None) => false,
		_ => true
	}}

	pub fn link(&self) -> Option<&str> {match self.0 {
		Some((ref s, _, _)) => Some(&s[..]),
		_ => None
	}}
	pub fn game(&self) -> Option<&str> {match self.0 {
		Some((_, Some(ref s), _)) => Some(&s[..]),
		_ => None
	}}
	pub fn start_time(&self) -> Option<DateTime<Local>> { match self.0{
		Some((_, _, ref time)) => Some(time.clone()),
		_ => None
	}}
}

impl Stream {
	pub fn get_status(&self, http: &mut Client) -> Result<StreamStatus, String> {
		match self {
			&Twitch(ref name) => http.get(&format!("https://api.twitch.tv/kraken/streams/{}", name)[..])
				.header(header::Accept(
					vec![header::qitem("application/vnd.twitchtv.v3+json".parse().ok().expect("couldnt parse MIME type"))]
				)).send().map_err(|_|"could not query api.twitch.tv".to_string())
				.and_then(|mut response| {
					if response.status != HttpOk {return Err(format!("{}", response.status))}
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
					if response.status != HttpOk {return Err(format!("{}", response.status))}
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
										Some(start) => NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M:%S")
											.map(|naive|Local.from_utc_datetime(&naive))
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
	pub fn path(&self) -> PathBuf {
		Path::new(match self {
			&Hitbox(_) => "hitbox",
			&Twitch(_) => "twitch"
		}).join(match self{&Twitch(ref x)|&Hitbox(ref x)=>x})
	}
	pub fn create_entry(&self) -> IoResult<()> {
		fs::File::create(&self.path()).map(|_| ())
	}
	pub fn remove_entry(&self) -> IoResult<()> {
		fs::remove_file(&self.path())
	}
	pub fn new(provider: &str, name: String) -> Option<Stream> {
		if name.contains('/') { return None }	// FIXME: maybe find a crossplatforn way to prevent filesystem access?
		match provider {
			"hitbox" | "hitbox.tv" => Some(Hitbox(name)),
			"twitch" | "twitch.tv" => Some(Twitch(name)),
			_ => None
		}
	}
}