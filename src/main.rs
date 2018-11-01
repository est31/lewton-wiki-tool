extern crate cmp;

extern crate hyper;
extern crate openssl;
extern crate hyper_openssl;
extern crate tokio;
extern crate md5;
extern crate percent_encoding;

extern crate structopt;

use std::io::{self, Write};

use hyper::{Client, Request, Body};
use hyper::header::USER_AGENT;
use hyper::rt::{Future, Stream};
use hyper::client::connect::{HttpConnector, Connect};
use hyper_openssl::HttpsConnector;
use openssl::ssl::{SslMethod, SslConnector};

use tokio::runtime::current_thread::Runtime;

use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};

use structopt::StructOpt;

// user agent to use
const AGENT :&str = "lewton wiki tool";

#[derive(StructOpt, Debug)]
//#[structopt]
enum Options {
	#[structopt(name = "get")]
	Get {
		name :String,
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

			let url = get_medium_url(&name);
			let url = url.parse::<hyper::Uri>().unwrap();

			let client = create_client();
			// Run the runtime with the future trying to fetch and print this URL.
			//
			// Note that in more complicated use cases, the runtime should probably
			// run on its own, and futures should just be spawned into it.
			let mut runtime = Runtime::new().unwrap();

			runtime.spawn(fetch_url(&client, url));

			runtime.run().unwrap();
		},
	}
}

fn get_medium_url(name :&str) -> String {
	let hash = md5::compute(name.as_bytes());
	let hash_hex = format!("{:x}", hash);

	let name_percent_encoded = utf8_percent_encode(name, DEFAULT_ENCODE_SET);
	let url = format!("https://upload.wikimedia.org/wikipedia/commons/{}/{}/{}",
		&hash_hex[..1], &hash_hex[..2], name_percent_encoded);
	println!("{}", url);
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

fn fetch_url<T :'static + Sync + Connect>(client :&Client<T>, url :hyper::Uri) -> impl Future<Item=(), Error=()> {

	let mut req = Request::builder();
	req.uri(url)
		.header(USER_AGENT, AGENT)
		.header("Range", "bytes=0-1023");

	client
		.request(req.body(Body::empty()).unwrap())
		// If we get a response back
		.and_then(|res| {
			println!("Response: {} success: {}", res.status(), res.status().is_success());
			println!("Response version: {:?}", res.version());
			println!("Headers: {:#?}", res.headers());

			// The body is a stream, and for_each returns a new Future
			// when the stream is finished, and calls the closure on
			// each chunk of the body...
			res.into_body().for_each(|chunk| {
				io::stdout().write_all(format!("chunk with length {}. ", chunk.len()).as_bytes())
					.map_err(|e| panic!("example expects stdout is open, error={}", e))
			})
		})
		// If all good, just tell the user...
		.map(|_| {
			println!("\n\nDone.");
		})
		// If there was an error, let the user know...
		.map_err(|err| {
			eprintln!("Error {}", err);
		})
}
