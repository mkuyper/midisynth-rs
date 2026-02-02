default: bwv1055-mov1-allegro.wav bwv1055-mov2-larghetto.wav bwv1055-mov3-allegro-manontanto.wav

MIDISYNTH := ../target/release/midisynth-rs

ifneq (, $(shell test -x "$(MIDISYNTH)" || echo nope))
$(error "$(MIDISYNTH)" is not executable or does not exist. Please run 'cargo build --release' at the top level)
endif

%.wav: %.mid | aegean/AegeanSymphonicOrchestra-v2_5-universal.sf2
	"$(MIDISYNTH)" --config aegean.toml $< $@

aegean/AegeanSymphonicOrchestra-v2_5-universal.sf2:
	$(MAKE) -C aegean

clean:
	rm -Rf *.wav

.PHONY: default clean
