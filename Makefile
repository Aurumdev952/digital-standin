# Digital Standin. Run `make help` for the list.
.DEFAULT_GOAL := help
BIN := target/release/standin

.PHONY: help setup asr-160 build run run-headless run-file demo play listen doctor test eval media chrome-check soak stop clean

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  make %-14s %s\n", $$1, $$2}'

setup: ## One-time setup: packages, camera, models, sidecar, build, clips
	scripts/setup.sh

asr-160: ## Optional: build the 160 ms speech model (words arrive about 0.16 s sooner)
	scripts/export-asr-160.sh

build: ## Build the release binary
	cargo build --release

run: build ## Start the control panel and the stand-in
	scripts/run.sh

run-headless: build ## Start without a window, printing what it does
	scripts/run.sh --headless

run-file: build ## Feed FILE instead of the meeting audio: make run-file FILE=test-media/meeting.wav
	scripts/run.sh --audio-file $(FILE)

demo: build ## 50 s demo with a laugh and a question to "Cindy" (no speakers needed)
	scripts/demo.sh

play: build ## Type state names to drive the camera by hand
	$(BIN) play

listen: build ## Record 5 s of the meeting audio to listen.wav
	$(BIN) listen

doctor: build ## Check camera, audio, models, GPU and sidecar
	$(BIN) doctor

test: ## Unit and integration tests
	cargo test --release --workspace

media: ## Download the public test recordings and build the labelled mixes
	scripts/fetch-test-media.sh
	python3 -I scripts/make-laugh-mix.py

eval: build ## Replay every labelled recording and score it (writes out/eval-summary.txt)
	scripts/eval.sh

chrome-check: ## Open the camera in headless Chrome like Meet does (needs the stand-in running)
	node scripts/chrome-camera-check.mjs 10

soak: build ## Long run on FILE through the muted speakers, sampling memory
	scripts/soak.sh $(FILE)

stop: ## Stop the stand-in and the decision model, freeing the GPU
	scripts/stop.sh

clean: ## Remove build output and replay results
	cargo clean
	rm -rf out
