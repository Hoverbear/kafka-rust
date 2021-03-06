extern crate kafka;
extern crate getopts;

use std::{env, fmt, process};
use std::fs::File;
use std::io::{self, stdin, Write, BufRead, BufReader};

use kafka::client::{KafkaClient, Compression};

// how many brokers do we require to acknowledge the send messages
const REQUIRED_ACKS: i16 = 1;
// how long do we allow wainting for the acknowledgement
const ACK_TIMEOUT: i32 = 100;

/// This is a very simple command line application sending every
/// non-empty line of standard input to a specified kafka topic; one
/// by one (not the most efficient way, of course).
///
/// Alternatively, messages can be read from an input file and sent do
/// kafka in batches (the typical use-case).
fn main() {
    let cfg = match Config::from_cmdline() {
        Ok(cfg) => cfg,
        Err(e) => {
            println!("{}", e);
            process::exit(1);
        }
    };
    if let Err(e) = produce(&cfg) {
        println!("{}", e);
        process::exit(1);
    }
}

fn produce(cfg: &Config) -> Result<(), Error> {
    let mut client = KafkaClient::new(cfg.brokers.clone());
    client.set_compression(cfg.compression);
    try!(client.load_metadata_all());

    // ~ verify that the remote brokers do know about the target topic
    if !client.topic_partitions.contains_key(&cfg.topic) {
        return Err(Error::Literal(format!("No such topic at {:?}: {}", cfg.brokers, cfg.topic)));
    }
    match cfg.input_file {
        None => {
            let stdin = stdin();
            let mut stdin = stdin.lock();
            produce_impl(&mut stdin, &mut client, &cfg)
        }
        Some(ref file) => {
            let mut r = BufReader::new(try!(File::open(file)));
            produce_impl(&mut r, &mut client, &cfg)
        }
    }
}

fn produce_impl(src: &mut BufRead, client: &mut KafkaClient, cfg: &Config) -> Result<(), Error> {
    let mut msg_buf = Vec::with_capacity(cfg.batch_size);
    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        if try!(src.read_line(&mut line_buf)) == 0 {
            break; // ~ EOF reached
        }
        let s = line_buf.trim();
        if s.is_empty() {
            continue; // ~ skip empty lines
        }
        // ~ buffer the line for later sending it as part of a bigger
        // batch
        msg_buf.push(kafka::utils::ProduceMessage {
            topic: cfg.topic.clone(),
            message: s.as_bytes().into(),
        });
        // ~ if we filled our batch send it out to kafka
        if msg_buf.len() >= cfg.batch_size {
            try!(client.send_messages(REQUIRED_ACKS, ACK_TIMEOUT, msg_buf));
            msg_buf = Vec::with_capacity(cfg.batch_size);
        }
    }
    // ~ flush pending messages
    if msg_buf.len() > 0 {
        try!(client.send_messages(REQUIRED_ACKS, ACK_TIMEOUT, msg_buf));
    }
    Ok(())
}

// --------------------------------------------------------------------

enum Error {
    Kafka(kafka::error::Error),
    Io(io::Error),
    Literal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &Error::Kafka(ref e) => write!(f, "kafka-error: {}", e),
            &Error::Io(ref e) => write!(f, "io-error: {}", e),
            &Error::Literal(ref s) => write!(f, "{}", s),
        }
    }
}

impl From<kafka::error::Error> for Error {
    fn from(e: kafka::error::Error) -> Self { Error::Kafka(e) }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self { Error::Io(e) }
}

// --------------------------------------------------------------------

struct Config {
    brokers: Vec<String>,
    topic: String,
    input_file: Option<String>,
    compression: Compression,
    batch_size: usize,
}

impl Config {
    fn from_cmdline() -> Result<Config, Error> {
        let args: Vec<String> = env::args().collect();
        let mut opts = getopts::Options::new();
        opts.optflag("h", "help", "Print this help screen");
        opts.optopt("", "brokers", "Specify kafka brokers (comma separated)", "HOSTS");
        opts.optopt("", "topic", "Specify target topic", "NAME");
        opts.optopt("", "input", "Specify input file", "FILE");
        opts.optopt("", "compression", "Compress messages [NONE, GZIP, SNAPPY]", "TYPE");
        opts.optopt("", "batch-size", "Send N message in one batch.", "N");

        let m = match opts.parse(&args[1..]) {
            Ok(m) => m,
            Err(e) => return Err(Error::Literal(e.to_string())),
        };
        if m.opt_present("help") {
            let brief = format!("{} [options]", args[0]);
            return Err(Error::Literal(opts.usage(&brief)));
        }
        Ok(Config {
            brokers: m.opt_str("brokers")
                .unwrap_or_else(|| "localhost:9092".to_owned())
                .split(',')
                .map(|s| s.trim().to_owned())
                .collect(),
            topic: m.opt_str("topic")
                .unwrap_or_else(|| "my-topic".to_owned()),
            input_file: m.opt_str("input"),
            compression: {
                let s = m.opt_str("compression")
                    .unwrap_or_else(|| "NONE".to_owned());
                match s.trim() {
                    "none" | "NONE" => Compression::NONE,
                    "gzip" | "GZIP" => Compression::GZIP,
                    "snappy" | "SNAPPY" => Compression::SNAPPY,
                    _ => return Err(Error::Literal(format!("Unknown compression type: {}", s))),
                }
            },
            batch_size: {
                match m.opt_str("batch-size") {
                    None => 1,
                    Some(s) => {
                        match s.parse::<usize>() {
                            Ok(n) => n,
                            Err(_) => return Err(Error::Literal(format!("Not a number: {}", s))),
                        }
                    }
                }
            },
        })
    }
}
