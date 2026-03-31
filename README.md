# Reproducing I2S RX bit-shift issues

On `esp-hal`, it seems that the RX stream is not aligned properly to the correct DIN frame boundaries. i.e.: each frame starts at a random bit within the stream causing the microphone readings to be blown up some power of 2 (for each bit offset).

## To Reproduce

This bug can be reproduced on this minimal reproducer repo though with some hardware setup on a physical ESP32-S3 devboard.


The idea is to:
1. Create a loopback mechanism on `I2S0` and physically wire `DIN` to `DOUT`
2. Start TX with a known repeating sequence
3. Wait/delay a bit
4. Read RX (loopback from TX)
5. Check to see if `RX == TX` and if not calculate the bit shift
6. Run this several times with different delays between starting TX and RX

Physical wiring for loopback:
```
GPIO6 (DIN) <-> GPIO18 (DOUT)
```

Then connect the devboard via USB and flash:
```
% cargo run --release
...
INFO - Creating I2S driver...
INFO - Starting I2S RX bit-shift reproduction harness
INFO - run 0: starting TX circular DMA
INFO - run 0: inserting 2ms delay before RX start
INFO - run 0: delay elapsed, starting RX DMA
INFO - run 0: bit offset = 26, marker_word_index = 3
INFO - run 0: TX circular DMA dropped
INFO - run 1: starting TX circular DMA
INFO - run 1: inserting 4ms delay before RX start
INFO - run 1: delay elapsed, starting RX DMA
INFO - run 1: bit offset = 24, marker_word_index = 7
INFO - run 1: TX circular DMA dropped
INFO - run 2: starting TX circular DMA
INFO - run 2: inserting 6ms delay before RX start
INFO - run 2: delay elapsed, starting RX DMA
INFO - run 2: bit offset = 1, marker_word_index = 5
INFO - run 2: TX circular DMA dropped
INFO - run 3: starting TX circular DMA
INFO - run 3: inserting 8ms delay before RX start
INFO - run 3: delay elapsed, starting RX DMA
INFO - run 3: bit offset = 9, marker_word_index = 0
INFO - run 3: TX circular DMA dropped
INFO - run 4: starting TX circular DMA
INFO - run 4: inserting 10ms delay before RX start
INFO - run 4: delay elapsed, starting RX DMA
INFO - run 4: bit offset = 31, marker_word_index = 5
INFO - run 4: TX circular DMA dropped
ERROR - FAIL: offsets = [Some(26), Some(24), Some(1), Some(9), Some(31)]
```
