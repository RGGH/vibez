use std::io::{self, Write};
use std::time::Duration;
use std::f32::consts::PI;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

//
// =========================
//   V O I C E
// =========================
//

#[derive(Clone, Copy, Debug)]
pub enum Waveform { Sine, Saw, Square, Triangle }

#[derive(Clone, Debug)]
pub struct Voice {
    phase: f32,
    frequency: f32,
    waveform: Waveform,
    amp: f32,
    // simple ADSR
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    env_phase: f32,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            frequency: 440.0,
            waveform: Waveform::Saw,
            amp: 0.15,
            attack: 0.01,
            decay: 0.1,
            sustain: 0.3,
            release: 0.1,
            env_phase: 0.0,
        }
    }

    pub fn set_frequency(&mut self, freq: f32) { self.frequency = freq; }

    pub fn process(&mut self, sample_rate: f32) -> f32 {
        let sample = match self.waveform {
            Waveform::Saw => 2.0 * (self.phase - 0.5),
            Waveform::Sine => (2.0 * PI * self.phase).sin(),
            Waveform::Square => if self.phase < 0.5 { 1.0 } else { -1.0 },
            Waveform::Triangle => 1.0 - (4.0 * (self.phase - 0.25)).abs(),
        };

        self.phase += self.frequency / sample_rate;
        if self.phase >= 1.0 { self.phase -= 1.0; }

        // simple envelope
        let env = if self.env_phase < self.attack {
            self.env_phase / self.attack
        } else if self.env_phase < self.attack + self.decay {
            1.0 - ((self.env_phase - self.attack)/self.decay)*(1.0 - self.sustain)
        } else {
            self.sustain
        };

        self.env_phase += 1.0 / sample_rate;

        sample * self.amp * env
    }

    pub fn reset_env(&mut self) { self.env_phase = 0.0; }
}

//
// =========================
//   S C A L E + UTILS
// =========================
//

fn midi_to_freq(n: i32) -> f32 { 440.0 * 2f32.powf((n as f32 - 69.0)/12.0) }

fn minor_scale(root: &str) -> Vec<i32> {
    let r = note_to_semitone(root);
    vec![0,2,3,5,7,8,10].iter().map(|x| x+r).collect()
}

fn note_to_semitone(name: &str) -> i32 {
    match name.to_lowercase().as_str() {
        "c"=>0,"c#"|"db"=>1,"d"=>2,"d#"|"eb"=>3,"e"=>4,"f"=>5,"f#"|"gb"=>6,
        "g"=>7,"g#"|"ab"=>8,"a"=>9,"a#"|"bb"=>10,"b"=>11,_=>0
    }
}

//
// =========================
//   S E Q U E N C E R
// =========================
//

#[derive(Clone, Debug)]
pub struct Sequencer {
    pub pattern: Vec<i32>,
    pub scale: Vec<i32>,
    pub octave: i32,
    pub transpose: i32,
    pub waveform: Waveform,
    pub voices: Vec<Voice>,

    pub sample_rate: f32,
    pub step: usize,
    pub samples_per_step: usize,
    pub sample_counter: usize,
}

impl Sequencer {
    pub fn new(sample_rate: f32) -> Self {
        let mut voices = Vec::new();
        for _ in 0..3 { voices.push(Voice::new()); }
        Self {
            pattern: vec![0],
            scale: minor_scale("g"),
            octave: 3,
            transpose: 0,
            waveform: Waveform::Saw,
            voices,
            sample_rate,
            step: 0,
            samples_per_step: (sample_rate/4.0) as usize,
            sample_counter: 0,
        }
    }

    pub fn process(&mut self) -> f32 {
        self.sample_counter += 1;
        if self.sample_counter >= self.samples_per_step {
            self.sample_counter = 0;
            self.step = (self.step + 1) % self.pattern.len();
            self.trigger_step();
        }

        // mix voices
        self.voices.iter_mut().map(|v| v.process(self.sample_rate)).sum::<f32>() / self.voices.len() as f32
    }

    fn trigger_step(&mut self) {
        let note = self.pattern[self.step % self.pattern.len()];
        let scale_note = self.scale[(note as usize) % self.scale.len()];
        let midi_base = scale_note + self.transpose + self.octave*12;

        for (i,v) in self.voices.iter_mut().enumerate() {
            let freq = midi_to_freq(midi_base + i as i32*7); // spread voices
            v.set_frequency(freq);
            v.waveform = self.waveform;
            v.reset_env();
        }
    }
}

//
// =========================
//   A U D I O
// =========================
//

fn play_audio(mut seq: Sequencer) {
    let host = cpal::default_host();
    let device = host.default_output_device().expect("no output device");
    let config = device.default_output_config().unwrap();
    let err_fn = |err| eprintln!("stream error: {err}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let cfg = config.clone().into();
            device.build_output_stream(&cfg, move |data: &mut [f32], _| {
                for s in data { *s = seq.process(); }
            }, err_fn, None).unwrap()
        }
        cpal::SampleFormat::I16 => {
            let cfg = config.clone().into();
            device.build_output_stream(&cfg, move |data: &mut [i16], _| {
                for s in data { *s = (seq.process()*i16::MAX as f32) as i16; }
            }, err_fn, None).unwrap()
        }
        cpal::SampleFormat::U16 => {
            let cfg = config.clone().into();
            device.build_output_stream(&cfg, move |data: &mut [u16], _| {
                for s in data {
                    let v = (seq.process()*0.5+0.5).clamp(0.0,1.0);
                    *s = (v*u16::MAX as f32) as u16;
                }
            }, err_fn, None).unwrap()
        }
        _ => panic!("Unsupported sample format"),
    };

    stream.play().unwrap();
    println!("🎶 Audio running. Ctrl+C to quit.");
    loop { std::thread::sleep(Duration::from_secs(1)); }
}

//
// =========================
//   P A R S E R
// =========================
//

fn parse_line(line: &str, seq: &mut Sequencer) {
    if line.starts_with("n\"") && line.ends_with("\"") {
        let inside = &line[2..line.len()-1];
        seq.pattern = inside.split_whitespace().filter_map(|x| x.parse::<i32>().ok()).collect();
    }
    if line.contains(".scale(") && line.contains("minor") {
        seq.scale = minor_scale("g");
    }
    if line.contains(".o(") {
        if let Ok(oct) = line.split(|c| c=='(' || c==')').nth(1).unwrap_or("3").parse() { seq.octave=oct; }
    }
    if line.contains(".trans(") {
        if let Ok(tr) = line.split(|c| c=='(' || c==')').nth(1).unwrap_or("0").parse() { seq.transpose=tr; }
    }
    if line.contains(".s(") {
        if line.contains("saw") { seq.waveform=Waveform::Saw; }
        if line.contains("sine") { seq.waveform=Waveform::Sine; }
        if line.contains("square") { seq.waveform=Waveform::Square; }
        if line.contains("triangle") { seq.waveform=Waveform::Triangle; }
    }
}

//
// =========================
//   M A I N
// =========================
//

fn main() {
    let mut seq = Sequencer::new(44100.0);

    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        parse_line(&args.join(" "), &mut seq);
    }

    // spawn audio thread
    let seq_clone = seq.clone();
    std::thread::spawn(move || { play_audio(seq_clone); });

    // REPL
    println!("=== vibez trance mode ===");
    println!("Example: $: n\"0 3 5 7 0 5 3 0\" .scale(\"g:minor\") .o(3) .s(\"saw\")");
    println!("Type 'quit' to exit\n");

    loop {
        print!("> ");
        io::stdout().flush().unwrap();
        let mut inp = String::new();
        io::stdin().read_line(&mut inp).unwrap();
        let inp = inp.trim();
        if inp=="quit" { break; }
        parse_line(inp, &mut seq);
    }
}

