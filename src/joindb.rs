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
use chrono::{Local, DateTime};
use serialize::json::{Json, ToJson, Object};
use std::collections::HashMap;
use std::path::Path;
use std::fs::File;
use std::io::prelude::*;

#[derive(Clone)]
pub struct JoinUser {
	pub num_visits: u64,
	pub last_visit: DateTime<Local>,
	pub first_visit: DateTime<Local>
}

impl JoinUser {
	fn from_json(json: &Json) -> Option<JoinUser> {
		let obj = tryopt!(json.as_object());
		let num = tryopt!(obj.get("num_visits").and_then(|json|json.as_u64()));
		let first = tryopt!(obj.get("first_visit").and_then(|json|json.as_string()));
		let last = tryopt!(obj.get("last_visit").and_then(|json|json.as_string()));
		let first_visit = tryopt!(first.parse().ok());
		let last_visit = tryopt!(last.parse().ok());
		Some(JoinUser {num_visits: num, first_visit: first_visit, last_visit: last_visit})
	}
}

impl ToJson for JoinUser {
	fn to_json(&self) -> Json {
		let mut obj = Object::new();
		obj.insert("num_visits".to_string(), Json::U64(self.num_visits));
		obj.insert("first_visit".to_string(), Json::String(self.first_visit.to_rfc3339()));
		obj.insert("last_visit".to_string(), Json::String(self.last_visit.to_rfc3339()));
		Json::Object(obj)
	}
}

pub enum JoinStatus{
	User(JoinUser),
	Alias(String)
}

impl ToJson for JoinStatus {
	fn to_json(&self) -> Json {
		let mut obj = Object::new();
		match self {
			&JoinStatus::User(ref user) => obj.insert("user".to_string(), user.to_json()),
			&JoinStatus::Alias(ref text) => obj.insert("alias".to_string(), Json::String(text.clone()))
		};
		Json::Object(obj)
	}
}

pub struct MemJoinDB(HashMap<String, JoinStatus>);

impl MemJoinDB {
	pub fn new() -> MemJoinDB {MemJoinDB(HashMap::new())}
}
impl FetchUpJoinDB for MemJoinDB {
	fn fetch(&self, name: &str) -> Option<JoinUser> {
		match self.0.get(name) {
			Some(&JoinStatus::Alias(ref name2)) => match self.0.get(name) {
				Some(&JoinStatus::User(ref u)) => Some(u.clone()),
				_ => None
			},
			Some(&JoinStatus::User(ref u)) => Some(u.clone()),
			None => None
		}
	}
	fn update(&mut self, name: &str, new: JoinStatus) -> String {
		let name2 = match self.0.get(name) {
			Some(&JoinStatus::Alias(ref name2)) => Some(name2.to_string()),	// this is really awkward, but we must not freeze the HashMap :(
			_ => None
		};
		if let Some(name2_) = name2 {
			self.0.insert(name2_.clone(), new);
			name2_
		} else {
			self.0.insert(name.to_string(), new);
			name.to_string()
		}
	}
}

trait FetchUpJoinDB {
	fn fetch(&self, name: &str) -> Option<JoinUser>;
	fn update(&mut self, name: &str, new: JoinStatus) -> String;
}

pub trait JoinDB {
	fn join(&mut self, name: &str) -> (String, Option<JoinUser>);
}
impl<DB: FetchUpJoinDB> JoinDB for DB {
	fn join(&mut self, name: &str) -> (String, Option<JoinUser>) {
		let now = Local::now();
		let (is_new, user) = self.fetch(name).map(|s|(false, s.clone())).unwrap_or_else(
			|| (true, JoinUser {num_visits: 0, first_visit: now, last_visit: now}));
		let name = self.update(name, JoinStatus::User(JoinUser {
			num_visits: user.num_visits+1, first_visit: user.first_visit, last_visit: now}));
		(name, if is_new {None} else {Some(user)})
	}
}

pub struct JsonJoinDB(String);

impl JsonJoinDB {
	pub fn new(dir: String) -> JsonJoinDB {JsonJoinDB(dir)}
}
impl FetchUpJoinDB for JsonJoinDB {
	fn fetch(&self, name: &str) -> Option<JoinUser> {
		let mut file = tryopt!(File::open(Path::join(&Path::new(&self.0), &Path::new(name))).ok());
		let mut s = String::new();
		file.read_to_string(&mut s).unwrap();
		let json = Json::from_str(&s).unwrap();
		let obj = json.as_object().unwrap();
		if let Some(user) = obj.get("user") {
			Some(JoinUser::from_json(user).unwrap())
		} else {
			None //Some(JoinStatus::Alias(obj.get("alias").unwrap().as_string().unwrap().to_string()))
		}
	}
	fn update(&mut self, name: &str, new: JoinStatus) -> String {
		use std::path::Path;
		let mut file = File::create(Path::join(&Path::new(&self.0), &Path::new(name))).unwrap();	// FIXME: will overwrite aliases
		write!(&mut file, "{}", new.to_json()).unwrap();
		name.to_string()
	}
}