extern crate cmp;
extern crate crossbeam_channel;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

extern crate hyper;
extern crate openssl;
extern crate hyper_openssl;
extern crate tokio;
extern crate md5;
extern crate percent_encoding;

extern crate structopt;

use std::io::Cursor;
use std::io::{BufRead, BufReader};
use std::fs::File;

use hyper::{Client, Request, Body};
use hyper::header::USER_AGENT;
use hyper::rt::{Future, Stream};
use hyper::client::connect::{HttpConnector, Connect};
use hyper_openssl::HttpsConnector;
use openssl::ssl::{SslMethod, SslConnector};

use tokio::runtime::Runtime;

use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};

use structopt::StructOpt;

use crossbeam_channel::Sender;

use tokio::prelude::future::{ok, Either};

use serde_json::to_string;

// user agent to use
const AGENT :&str = "lewton wiki tool";

#[derive(StructOpt, Debug)]
enum Options {
	#[structopt(name = "get")]
	Get {
		name :String,
	},
	#[structopt(name = "get-list")]
	GetList {
		list_path :String,
		#[structopt(name = "j")]
		jobs :Option<usize>,
	},
	#[structopt(name = "show-url")]
	ShowUrl {
		name :String,
	},
}

fn main() {
	let options = Options::from_args();

	match options {
		Options::ShowUrl { name } => {
			let url = get_medium_url(&name);
			println!("URL is: {}", url);
		},
		Options::Get { name } => {
			let client = create_client();

			let mut runtime = Runtime::new().unwrap();

			let url = get_medium_url(&name);
			let url = url.parse::<hyper::Uri>().unwrap();
			runtime.spawn(fetch_url_verbose(&client, url));

			runtime.shutdown_on_idle()
				.wait().unwrap();
		},
		Options::GetList { list_path, jobs } => {
			println!("opening list file {}", list_path);
			let f = File::open(list_path).unwrap();
			let mut br = BufReader::new(f);
			let mut runtime = Runtime::new().unwrap();

			let client = create_client();

			let (s, r) = crossbeam_channel::unbounded();

			std::thread::spawn(move || {
				while let Some(msg) =  r.recv() {
					println!("{}", to_string(&msg).unwrap());
				}
			});

			for l in br.lines() {
				let name = l.unwrap();
				runtime.spawn(fetch_name(&client, name, s.clone()));
			}
			runtime.shutdown_on_idle()
				.wait().unwrap();
		}
	}
}


#[derive(Debug, Serialize)]
struct RequestRes {
	/// Filename that was requested
	file_name :String,
	/// Result payload
	result_kind :RequestResKind,
}

#[derive(Debug, Serialize)]
enum RequestResKind {
	/// Successful response, with comparison result inside
	Success(Result<(usize, usize), String>),
	/// Got a response but with wrong status code
	WrongResponse(u16),
	/// Error during obtaining a response
	Error(String),
}

fn get_medium_url(name :&str) -> String {
	let hash = md5::compute(name.as_bytes());
	let hash_hex = format!("{:x}", hash);

	let name_percent_encoded = utf8_percent_encode(name, DEFAULT_ENCODE_SET);
	let url = format!("https://upload.wikimedia.org/wikipedia/commons/{}/{}/{}",
		&hash_hex[..1], &hash_hex[..2], name_percent_encoded);
	url
}

/// Creates a client that performs https requests
fn create_client() -> Client<HttpsConnector<HttpConnector>> {
	let mut ssl = SslConnector::builder(SslMethod::tls()).unwrap();
	ssl.set_alpn_protos(b"\x02h2").unwrap();
	let mut http = HttpConnector::new(4);
	http.enforce_http(false);
	let https = HttpsConnector::with_connector(http, ssl).unwrap();

	let client = Client::builder()
		.http2_only(true)
		.build::<_, hyper::Body>(https);
	client
}

fn fetch_url_verbose<T :'static + Sync + Connect>(client :&Client<T>, url :hyper::Uri) -> impl Future<Item=(), Error=()> {

	let mut req = Request::builder();
	req.uri(url)
		.header(USER_AGENT, AGENT);

	client
		.request(req.body(Body::empty()).unwrap())
		.and_then(|res| {
			println!("Response: {} success: {}", res.status(), res.status().is_success());
			println!("Response version: {:?}", res.version());
			println!("Headers: {:#?}", res.headers());

			let is_success = res.status().is_success();
			res.into_body().concat2().map(move |body| {
				if is_success {
					println!("body is {} characters long", body.len());
					let cursor1 = Cursor::new(&body);
					let cursor2 = Cursor::new(&body);
					let res = cmp::cmp_output(cursor1, cursor2);
					println!("Comparison result: {:?}", res);
				}
			})
		})
		.map(|_| {
			println!("\n\nDone.");
		})
		.map_err(|err| {
			eprintln!("Error {}", err);
		})
}

fn fetch_name<T :'static + Sync + Connect>(client :&Client<T>, name :String, sender :Sender<RequestRes>) -> impl Future<Item=(), Error=()> {

	let url = get_medium_url(&name);
	let url = url.parse::<hyper::Uri>().unwrap();

	let mut req = Request::builder();

	req.uri(url)
		.header(USER_AGENT, AGENT);

	let send_kind = move |result_kind| {
		sender.send(RequestRes {
			file_name : name,
			result_kind,
		});
	};
	client
		.request(req.body(Body::empty()).unwrap())
		.then(move |res| {
			match res {
				Ok(res) => {
					let status = res.status();
					Either::A(res.into_body().concat2().map(move |body| {
						if status.is_success() {
							let cursor1 = Cursor::new(&body);
							let cursor2 = Cursor::new(&body);
							let res = cmp::cmp_output(cursor1, cursor2);
							send_kind(RequestResKind::Success(res));
						} else {
							send_kind(RequestResKind::WrongResponse(status.as_u16()));
						}
					}))
				},
				Err(err) => {
					send_kind(RequestResKind::Error(format!("{}", err)));
					Either::B(ok(()))
				},
			}
		})
		.map_err(|_err| {})
}
