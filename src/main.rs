extern crate cmp;
#[macro_use]
extern crate crossbeam_channel;
#[macro_use]
extern crate serde;
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

use hyper::{Client, Request, Body, StatusCode};
use hyper::header::USER_AGENT;
use hyper::rt::{Future, Stream};
use hyper::client::connect::{HttpConnector, Connect};
use hyper_openssl::HttpsConnector;
use openssl::ssl::{SslMethod, SslConnector};

use tokio::runtime::Runtime;

use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};

use structopt::StructOpt;

use crossbeam_channel::Sender;

use tokio::prelude::future::ok;

use tokio::prelude::future::Either;

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
					println!("{:?}", msg);
				}
			});

			for l in br.lines() {
				let name = l.unwrap();
				let url = get_medium_url(&name);
				let url = url.parse::<hyper::Uri>().unwrap();
				runtime.spawn(fetch_url(&client, url, s.clone()));
			}
			runtime.shutdown_on_idle()
				.wait().unwrap();
		}
	}
}

#[derive(Debug)]
enum RequestResult {
	Success(Result<(usize, usize), String>),
	// Got a response but with wrong status code
	WrongResponse(StatusCode),
	// Error during obtaining a response
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

fn fetch_url<T :'static + Sync + Connect>(client :&Client<T>, url :hyper::Uri, sender :Sender<RequestResult>) -> impl Future<Item=(), Error=()> {

	let mut req = Request::builder();

	req.uri(url)
		.header(USER_AGENT, AGENT);

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
							sender.send(RequestResult::Success(res));
						} else {
							sender.send(RequestResult::WrongResponse(status));
						}
					}))
				},
				Err(err) => {
					sender.send(RequestResult::Error(format!("{}", err)));
					Either::B(ok(()))
				},
			}
		})
		.map_err(|err| {})
}
