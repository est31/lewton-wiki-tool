extern crate cmp;
extern crate ogg;

extern crate hyper;
extern crate openssl;
extern crate hyper_openssl;
extern crate tokio;
extern crate md5;
extern crate percent_encoding;

extern crate structopt;

use ogg::PacketReader;

use std::io::Cursor;

use hyper::{Client, Request, Body};
use hyper::header::USER_AGENT;
use hyper::rt::{Future, Stream};
use hyper::client::connect::{HttpConnector, Connect};
use hyper_openssl::HttpsConnector;
use openssl::ssl::{SslMethod, SslConnector};

use tokio::runtime::current_thread::Runtime;

use percent_encoding::{utf8_percent_encode, DEFAULT_ENCODE_SET};

use structopt::StructOpt;

use ogg_metadata_extracted::BareOggFormat;

// user agent to use
const AGENT :&str = "lewton wiki tool";

#[derive(StructOpt, Debug)]
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

fn get_ogg_format_of_stream(content :&[u8]) -> Result<BareOggFormat, String> {
	let rdr = Cursor::new(content);
	let mut pck_rdr = PacketReader::new(rdr);
	let pck = try!(pck_rdr.read_packet_expected().map_err(|e| format!("{}", e)));
	let format = try!(ogg_metadata_extracted::identify_packet_data_by_magic(&pck.data).ok_or("Unrecognized format"));
	Ok(format.1)
}

/// Stuff taken from the ogg_metadata crate's source code
mod ogg_metadata_extracted {

	#[derive(Debug, Copy, Clone, PartialEq)]
	pub enum BareOggFormat {
		Vorbis,
		Opus,
		Theora,
		Speex,
		Skeleton,
	}

	pub fn identify_packet_data_by_magic(pck_data :&[u8]) -> Option<(usize, BareOggFormat)> {
		// Magic sequences.
		// https://www.xiph.org/vorbis/doc/Vorbis_I_spec.html#x1-620004.2.1
		let vorbis_magic = &[0x01, 0x76, 0x6f, 0x72, 0x62, 0x69, 0x73];
		// https://tools.ietf.org/html/rfc7845#section-5.1
		let opus_magic = &[0x4f, 0x70, 0x75, 0x73, 0x48, 0x65, 0x61, 0x64];
		// https://www.theora.org/doc/Theora.pdf#section.6.2
		let theora_magic = &[0x80, 0x74, 0x68, 0x65, 0x6f, 0x72, 0x61];
		// http://www.speex.org/docs/manual/speex-manual/node8.html
		let speex_magic = &[0x53, 0x70, 0x65, 0x65, 0x78, 0x20, 0x20, 0x20];
		// https://wiki.xiph.org/Ogg_Skeleton_4#Ogg_Skeleton_version_4.0_Format_Specification
		let skeleton_magic = &[0x66, 105, 115, 104, 101, 97, 100, 0];

		if pck_data.len() < 1 {
			return None;
		}

		use self::BareOggFormat::*;
		let ret :(usize, BareOggFormat) = match pck_data[0] {
			0x01 if pck_data.starts_with(vorbis_magic) => (vorbis_magic.len(), Vorbis),
			0x4f if pck_data.starts_with(opus_magic) => (opus_magic.len(), Opus),
			0x80 if pck_data.starts_with(theora_magic) => (theora_magic.len(), Theora),
			0x53 if pck_data.starts_with(speex_magic) => (speex_magic.len(), Speex),
			0x66 if pck_data.starts_with(skeleton_magic) => (speex_magic.len(), Skeleton),

			_ => return None,
		};

		return Some(ret);
	}
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

			res.into_body().concat2().map(|body| {
				println!("body is {} characters long", body.len());
				//body.
				let format = get_ogg_format_of_stream(&body);
				println!("format is: {:?}", format);

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
