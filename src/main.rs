// Copyright (C) 2026 Mike Kuyper <mike@kuyper.us>. All rights reserved.
//
// This file is subject to the terms and conditions defined in file 'LICENSE',
// which is part of this source code package.

use std::io::Read;

struct SequencedTrack<'a> {
    track: std::vec::IntoIter<midly::TrackEvent<'a>>,
    next: Option<midly::TrackEvent<'a>>,
    ticks: u32,
    count: usize,
}

impl<'a> SequencedTrack<'a> {
    fn create(track: midly::Track<'a>) -> Self {
        let count = track.len();
        let mut track = Self {
            track: track.into_iter(),
            next: None,
            ticks: 0,
            count,
        };

        track.advance();

        track
    }

    fn advance(&mut self) {
        self.next = self.track.next();
        self.ticks = self
            .next
            .map_or(u32::MAX, |e| self.ticks + u32::from(e.delta));
    }
}

#[derive(Debug)]
struct SequencerEvent<'a> {
    idx: usize,
    ticks: u32,
    event: midly::TrackEvent<'a>,
}

#[derive(Clone, Debug)]
struct PlayerEvent {
    time: usize,
    note: u8,
    velocity: u8,
}

#[derive(Clone, Debug, Default)]
struct PlayerTrack {
    name: Option<String>,
    length: usize,
    events: Vec<PlayerEvent>,
}

struct Sequencer<'a> {
    tracks: Vec<SequencedTrack<'a>>,
}

impl<'a> Sequencer<'a> {
    fn next(&mut self) -> Option<SequencerEvent<'a>> {
        let (idx, ticks) = self
            .tracks
            .iter()
            .map(|t| t.ticks)
            .enumerate()
            .min_by_key(|v| v.1)?;

        if ticks != u32::MAX {
            let track = &mut self.tracks[idx];

            let event = track.next.unwrap();
            track.advance();

            Some(SequencerEvent { idx, ticks, event })
        } else {
            None
        }
    }

    fn play_all(
        &mut self,
        timing: midly::Timing,
        pbar: indicatif::ProgressBar,
    ) -> Vec<PlayerTrack> {
        let mut tracks: Vec<_> = std::iter::repeat_with(|| PlayerTrack::default())
            .take(self.tracks.len())
            .collect();

        let ticks_per_beat = match timing {
            midly::Timing::Metrical(tpb) => usize::from(tpb.as_int()),
            _ => todo!(),
        };
        let mut tempo: usize = 500_000;

        let mut base_time: usize = 0;
        let mut base_ticks: u32 = 0;

        pbar.set_length(self.tracks.iter().map(|t| t.count).sum::<usize>() as u64);

        while let Some(e) = self.next() {
            let delta_ticks = e.ticks - base_ticks;
            let delta_time = (delta_ticks as usize) * tempo / ticks_per_beat;

            let time = base_time + delta_time;

            match e.event.kind {
                midly::TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) => {
                    tempo = t.as_int() as usize;

                    base_time += delta_time;
                    base_ticks += delta_ticks;
                }
                midly::TrackEventKind::Meta(midly::MetaMessage::TrackName(n)) => {
                    tracks[e.idx].name = String::from_utf8(n.to_vec()).ok();
                }
                midly::TrackEventKind::Meta(midly::MetaMessage::EndOfTrack) => {
                    tracks[e.idx].length = time;
                }
                midly::TrackEventKind::Midi {
                    channel: _,
                    message: midly::MidiMessage::NoteOn { key: k, vel: v },
                } => {
                    tracks[e.idx].events.push(PlayerEvent {
                        time,
                        note: k.into(),
                        velocity: v.into(),
                    });
                }
                _ => { /* println!("skipping: {:?}", e); */ }
            };
            pbar.inc(1);
        }

        pbar.finish_and_clear();

        tracks
    }
}

#[derive(Debug)]
struct InstrumentSetting {
    bank: u8,
    preset: u8,
    transpose: Option<i8>,
    pan: Option<f32>,  // -1 .. 1
    gain: Option<f32>, // dB
}

impl InstrumentSetting {
    fn from_toml(setting: &toml::Value) -> Result<Self, String> {
        Ok(Self {
            bank: setting
                .get("bank")
                .and_then(|v| v.as_integer())
                .and_then(|v| Some(v as u8))
                .ok_or("Missing bank value")?,

            preset: setting
                .get("preset")
                .and_then(|v| v.as_integer())
                .and_then(|v| Some(v as u8))
                .ok_or("Missing preset value")?,

            transpose: setting
                .get("tsp")
                .and_then(|v| v.as_integer())
                .and_then(|v| Some(v as i8)),

            pan: setting.get("pan").and_then(|v| match v {
                toml::Value::Integer(i) => Some(*i as f32),
                toml::Value::Float(f) => Some(*f as f32),
                _ => None,
            }),

            gain: setting.get("gain").and_then(|v| match v {
                toml::Value::Integer(i) => Some(*i as f32),
                toml::Value::Float(f) => Some(*f as f32),
                _ => None,
            }),
        })
    }
}

struct Renderer {
    synth: rustysynth::Synthesizer,
    track: PlayerTrack,
}

impl Renderer {
    fn render(
        &mut self,
        instr: &InstrumentSetting,
        padding: usize,
        pbar: indicatif::ProgressBar,
    ) -> (Vec<f32>, Vec<f32>) {
        let sr: usize = self.synth.get_sample_rate() as usize;
        let bs: usize = self.synth.get_block_size();

        let sc: usize = ((self.track.length + padding) * sr / 1_000_000).next_multiple_of(bs);

        let mut left: Vec<f32> = vec![0_f32; sc];
        let mut right: Vec<f32> = vec![0_f32; sc];

        let mut it = self.track.events.iter().peekable();

        // Setup instruments using MIDI control messages, as Synthesizer has no API for this.
        self.synth
            .process_midi_message(0, 0xb0, 0x00, instr.bank.into());
        self.synth
            .process_midi_message(0, 0xc0, instr.preset.into(), 0);
        let transpose = instr.transpose.unwrap_or(0);

        pbar.set_length(sc as u64);

        for si in (0..sc).step_by(bs) {
            let t = (si * 1_000_000) / sr;

            loop {
                if let Some(e) = it.peek() {
                    if e.time <= t {
                        let note = e.note.strict_add_signed(transpose);
                        self.synth.note_on(0, note.into(), e.velocity.into());
                        it.next();
                        continue;
                    }
                }
                break;
            }

            self.synth
                .render(&mut left[si..si + bs], &mut right[si..si + bs]);

            pbar.inc(bs as u64);
        }
        pbar.finish_and_clear();

        (left, right)
    }
}

struct MixerGainFactors {
    l_to_l: f32,
    l_to_r: f32,
    r_to_l: f32,
    r_to_r: f32,
}

impl MixerGainFactors {
    fn new(gain_db: f32, pan: f32) -> Self {
        // map pan from [-1 .. 1] to [0 .. π/2] (radians)
        let pan_rad = (((pan % 1.0) + 1.0) / 2.0) * (std::f32::consts::PI / 2.0);
        // convert gain from dB to factor
        let gain = 10f32.powf(gain_db / 20.0);

        Self {
            // destination: left (cos curve)
            l_to_l: gain * pan_rad.cos(),
            r_to_l: gain * (pan_rad + (std::f32::consts::PI / 4.0)).cos().max(0.0),

            // destination: right (sin curve)
            r_to_r: gain * pan_rad.sin(),
            l_to_r: gain * (pan_rad - (std::f32::consts::PI / 4.0)).sin().max(0.0),
        }
    }
}

struct MixerTrack {
    left: Vec<f32>,
    right: Vec<f32>,
    gainfactors: MixerGainFactors,
}

struct Mixer {
    tracks: Vec<MixerTrack>,
}

impl Mixer {
    fn mix_stereo(&self, pbar: indicatif::ProgressBar) -> Vec<f32> {
        let sc = self.tracks.iter().map(|t| t.left.len()).max().unwrap();

        let mut out: Vec<f32> = Vec::with_capacity(sc * 2);

        pbar.set_length(sc as u64);

        for si in 0..sc {
            let mut sl: f32 = 0f32; // output sample accumulator to left channel
            let mut sr: f32 = 0f32; // output sample accumulator to right channel
            for t in self.tracks.iter() {
                let gf = &t.gainfactors;
                let il = t.left.get(si).copied().unwrap_or(0.0); // input from left channel
                let ir = t.right.get(si).copied().unwrap_or(0.0); // input from right channel
                sl += gf.l_to_l * il + gf.r_to_l * ir;
                sr += gf.l_to_r * il + gf.r_to_r * ir;
            }
            out.push(sl);
            out.push(sr);

            pbar.inc(1);
        }

        pbar.finish_and_clear();

        out
    }
}

mod args {
    #[derive(clap::Parser)]
    #[command(author, version)]
    pub struct Args {
        /// Configuration file
        #[arg(short, long)]
        pub config: clio::Input,

        /// Input MIDI file
        pub midifile: clio::Input,

        /// Destination WAV file
        pub wavfile: clio::OutputPath,
    }
}

fn midisynth() -> Result<(), String> {
    let mut args = <args::Args as clap::Parser>::parse();

    // Prepare progress bar style and UI elements
    let sty = indicatif::ProgressStyle::with_template("      {bar:40.cyan/blue} {msg}")
        .unwrap()
        .progress_chars("#>-");
    let warning = console::style("Warning").yellow().bold();

    // Parse configuration
    let mut s = String::new();
    args.config
        .read_to_string(&mut s)
        .map_err(|e| format!("Reading configuration file {} failed: {}", args.config, e))?;
    let config = s
        .parse::<toml::Table>()
        .map_err(|e| format!("Parsing configuration file {} failed: {}", args.config, e))?;
    let instr = config
        .get("instr")
        .and_then(|v| v.as_table())
        .ok_or("Invalid configuration: No instruments specified")?;

    // Load sound font
    let sf_fname = config
        .get("soundfont")
        .and_then(|v| v.as_str())
        .ok_or("Invalid configuration: No soundfont specified")?;
    let mut sf_file = std::fs::File::open(sf_fname)
        .map_err(|e| format!("Opening soundfont file {} failed: {}", sf_fname, e))?;
    let sf_object = std::sync::Arc::new(
        rustysynth::SoundFont::new(&mut sf_file)
            .map_err(|e| format!("Loading soundfont file {} failed: {}", sf_fname, e))?,
    );

    // Load MIDI file
    let mut mf_data = Vec::new();
    args.midifile
        .read_to_end(&mut mf_data)
        .map_err(|e| format!("Reading MIDI file {} failed: {}", args.midifile, e))?;
    let mf_object = midly::Smf::parse(&mf_data)
        .map_err(|e| format!("Loading MIDI file {} failed: {}", args.midifile, e))?;

    // Sequence MIDI file
    println!("[1/3] Sequencing MIDI file...");
    let pbar = indicatif::ProgressBar::no_length();
    pbar.set_style(sty.clone());

    let mut seq = Sequencer {
        tracks: mf_object
            .tracks
            .into_iter()
            .map(|t| SequencedTrack::create(t))
            .collect(),
    };

    let tracks = seq.play_all(mf_object.header.timing, pbar);

    // Render tracks
    let mpbar = indicatif::MultiProgress::new();
    mpbar.println("[2/3] Rendering tracks...").ok();

    let mut threads = Vec::new();

    for (idx, track) in tracks.into_iter().enumerate() {
        if let Some(ref track_name) = track.name {
            let settings = match instr.get(track_name).and_then(|v| v.as_array()) {
                Some(s) => s,
                None => {
                    if idx != 0 {
                        mpbar
                            .println(format!(
                                "      {}: No instruments defined for {}, skipping track!",
                                warning, track_name
                            ))
                            .ok();
                    }
                    continue;
                }
            };

            for setting in settings {
                let is = match InstrumentSetting::from_toml(setting) {
                    Ok(is) => is,
                    Err(msg) => {
                        mpbar
                            .println(format!(
                                "      {}: {msg} for {}, skipping track!",
                                warning, track_name
                            ))
                            .ok();
                        continue;
                    }
                };

                let synth_settings = rustysynth::SynthesizerSettings::new(44100);
                let synth_object =
                    rustysynth::Synthesizer::new(&sf_object, &synth_settings).unwrap();

                let pbar = mpbar.add(indicatif::ProgressBar::no_length());
                pbar.set_style(sty.clone());
                pbar.set_message(track_name.clone());

                let mut renderer = Renderer {
                    synth: synth_object,
                    track: track.clone(),
                };

                let thread_handle = std::thread::spawn(move || {
                    let (left, right) = renderer.render(&is, 1_500_000, pbar);

                    MixerTrack {
                        left,
                        right,
                        gainfactors: MixerGainFactors::new(
                            is.gain.unwrap_or(0f32),
                            is.pan.unwrap_or(0f32),
                        ),
                    }
                });

                threads.push(thread_handle);
            }
        }
    }

    if threads.len() == 0 {
        return Err("No music was generated".into());
    }

    let mtracks = threads.into_iter().map(|t| t.join().unwrap()).collect();

    // Mix tracks
    println!("[3/3] Mixing...");
    let pbar = indicatif::ProgressBar::no_length();
    pbar.set_style(sty.clone());

    let mixer = Mixer { tracks: mtracks };
    let wavdata = mixer.mix_stereo(pbar);

    let wav_fname: &std::path::Path = args.wavfile.path();
    wavers::write(wav_fname, &wavdata, 44100, 2)
        .map_err(|e| format!("Writing output WAV file {} failed: {}", args.wavfile, e))?;

    Ok(())
}

fn main() {
    if let Err(msg) = midisynth() {
        println!("{}: {}", console::style("Error").red().bold(), msg);
    }
}
