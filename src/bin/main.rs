#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::i2s::master::{Channels, Config as I2sConfig, DataFormat, I2s};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{dma_buffers, dma_circular_buffers_chunk_size};
use log::{error, info};

const DMA_BYTES: usize = 4096;
const DMA_CHUNK_SIZE: usize = 1024;
const RX_CAPTURE_BYTES: usize = 4092;
const RUN_COUNT: usize = 5;
const TX_PATTERN: [u32; 8] = [
    0xDEAD_0001,
    0xBEEF_0002,
    0xCAFE_0003,
    0xF00D_0004,
    0x0000_0005,
    0x0000_0006,
    0x0000_0007,
    0x0000_0008,
];

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main(stack_size = 10240)]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    // Step 0: Set up - initialize peripherals, dma, etc.
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    let (rx_buffer, rx_descriptors, _, _) = dma_buffers!(DMA_BYTES, 0);
    let (_, _, tx_buffer, tx_descriptors) =
        dma_circular_buffers_chunk_size!(0, DMA_BYTES, DMA_CHUNK_SIZE);

    // Step 1: Create a loopback mechanism on `I2S0` with GPIOs 4/5 as BCLK/WS, GPIO 18 as TX data, and GPIO 6 as RX data.
    info!("Creating I2S driver...");
    let i2s = I2s::new(
        peripherals.I2S0,
        peripherals.DMA_CH0,
        I2sConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(44_100))
            .with_data_format(DataFormat::Data32Channel32)
            .with_channels(Channels::STEREO),
    )
    .expect("I2S::new failed")
    .into_async();

    let bclk = peripherals.GPIO4;
    let ws = peripherals.GPIO5;
    // TX and RX share BCLK and WS pins 
    // Should be OK given ESP32-S3 the GPIO matrix can simultaneously drive a pin as output and loop it back as input - but still worth noting that this is a bit of an unusual configuration.
    // Ideally, we would want to use I2S0 as master and I2S1 as slave - but currently I2S slave mode is not supported
    let tx_bclk = unsafe { bclk.clone_unchecked() };
    let tx_ws = unsafe { ws.clone_unchecked() };

    let mut i2s_tx = i2s
        .i2s_tx
        .with_bclk(tx_bclk)
        .with_ws(tx_ws)
        .with_dout(peripherals.GPIO18)
        .build(tx_descriptors);

    let mut i2s_rx = i2s
        .i2s_rx
        .with_bclk(bclk)
        .with_ws(ws)
        .with_din(peripherals.GPIO6)
        .build(rx_descriptors);

    fill_tx_buffer(tx_buffer);

    info!("Starting I2S RX bit-shift reproduction harness");

    let mut observed_offsets = [None; RUN_COUNT];
    let mut observed_word_indices = [None; RUN_COUNT];

    for run in 0..RUN_COUNT {
        let delay_ms = ((run as u32) + 1) * 2;
        rx_buffer.fill(0);

        // Step 2. Start TX with a known repeating sequence
        info!("run {}: starting TX circular DMA", run);
        let tx_transfer = i2s_tx
            .write_dma_circular(&*tx_buffer)
            .expect("TX circular DMA failed to start");

        // Step 3. Wait/delay a bit
        info!(
            "run {}: inserting {}ms delay before RX start",
            run, delay_ms
        );

        Timer::after(Duration::from_millis(delay_ms as u64)).await;
        info!("run {}: delay elapsed, starting RX DMA", run);

        // Step 4. Read RX
        i2s_rx
            .read_dma_async(&mut rx_buffer[..RX_CAPTURE_BYTES])
            .await
            .expect("RX DMA failed to start");

        // Step 5. Check to see if `RX == TX` and if not calculate the bit shift
        let alignment = detect_alignment(&rx_buffer[..RX_CAPTURE_BYTES]);
        observed_offsets[run] = alignment.map(|(offset, _)| offset);
        observed_word_indices[run] = alignment.map(|(_, word_index)| word_index);

        match alignment {
            Some((offset, word_index)) => info!(
                "run {}: bit offset = {}, marker_word_index = {}",
                run, offset, word_index
            ),
            None => {
                error!(
                    "run {}: marker not found — severe misalignment or data loss",
                    run
                );
            }
        }

        // Step 6. Stop TX and repeat several times (with different delays)
        drop(tx_transfer);
        info!("run {}: TX circular DMA dropped", run);
    }

    let all_pass = 
    // (no sub-word bit drift)
    observed_offsets.iter().all(|o| *o == Some(0))
    // (L/R channel assignment is correct, not swapped)
        && observed_word_indices
            .iter()
            .all(|i| i.map(|w| w % 2 == 0) == Some(true));

    if all_pass {
        info!("PASS: offsets = {:?}", observed_offsets);
    } else {
        error!("FAIL: offsets = {:?}", observed_offsets);
    }

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

fn fill_tx_buffer(tx_buffer: &mut [u8]) {
    for (word_index, chunk) in tx_buffer.chunks_exact_mut(4).enumerate() {
        let pattern_word = TX_PATTERN[word_index % TX_PATTERN.len()];
        chunk.copy_from_slice(&pattern_word.to_le_bytes());
    }
}

// "Shift' meaning: the TX pattern starts <shift> bits into word_index, i.e., reconstructed as (rx[n] << (32 - shift)) | (rx[n+1] >> shift)
fn detect_alignment(buf: &[u8]) -> Option<(u32, usize)> {
    let word_count = buf.len() / 4;
    if word_count < TX_PATTERN.len() + 1 {
        return None;
    }

    for word_index in 0..word_count {
        for shift in 0u32..32 {
            let mut matched = true;

            for (pattern_idx, expected) in TX_PATTERN.iter().enumerate() {
                let curr = read_u32_word(buf, (word_index + pattern_idx) % word_count);
                let next = read_u32_word(buf, (word_index + pattern_idx + 1) % word_count);

                let candidate = if shift == 0 {
                    curr
                } else {
                    (curr << (32 - shift)) | (next >> shift)
                };

                if candidate != *expected {
                    matched = false;
                    break;
                }
            }

            if matched {
                return Some((shift, word_index));
            }
        }
    }

    None
}

fn read_u32_word(buf: &[u8], word_index: usize) -> u32 {
    let base = word_index * 4;
    u32::from_le_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]])
}
