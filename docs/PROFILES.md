# Link profiles: the library, version 1

A **profile** is a named set of values for a link, so that a test can say `profile: lte-typical` and not have to know what LTE is. The decision is [D-095](decisions/D-095.md); the commands are `netscenario profiles` and `netscenario capabilities` ([CLI.md](CLI.md)).

```yaml
topology:
  nodes: [phone, server]
  links:
    - id: access
      between: [phone, server]       # the first node is the terminal: forward is up, reverse is down
      profile: lte-good
      forward: { loss: { model: random, p: 1% } }   # the keys written here win over the profile
```

## The rule of the library

**A profile sets a key only if a source states it**, or if it follows from sources by arithmetic that is shown on this page. Nothing in the library is an estimate, and nothing is marked as "a choice" either: a number that is marked is still a number that a test will take for a fact. What no source gives is **not set**; the author writes it next to the profile. A tier, a family or a key that has no source waits for one, and arrives with a new version of the library (see "What is not in version 1" below, which says what exists and why it is not enough).

That makes the library small, and mostly delays. A delay is what most sources publish (a round trip to a test server); a loss, a jitter or a rate that is a property of the *link* and not of one test almost never is.

## How to read it

- **Delay is one-way.** A source gives a round trip, so the value is **half of it**. That assumes the two directions are alike (a profile with different directions says so), and it holds a share of the path beyond the link, because the test server is not at the access point: the delay of a profile is an upper bound for the link itself, which is the safe side for testing a program.
- **Up and down.** `up` is from the terminal to the network (`forward` of a link whose first node in `between` is the terminal), `down` the other way. A profile with one value for both has `both`.
- **Tiers are named after what the sources measured**, not guessed. `good` is the lowest published median of the sources used for the family, `poor` the highest, `typical` a national or global median. A tier with one source is a tier of one source, and its row says which.
- **Dates matter.** The numbers are medians or averages of a period; the column says which. A newer source replaces an older one in a new version of the library.
- **✓** at a source: the figure was compared with the text of the paper or the page when this page was written.

## The profiles

| Profile | Up | Down | What it is | Source |
| --- | --- | --- | --- | --- |
| `3g-good` | delay 15 ms | delay 18 ms | HSPA, medians of the one-way delay, Vienna 2012 | S1 |
| `3g-typical` | delay 21.15 ms | delay 21.15 ms | 3G in the UK, average round trip 42.3 ms, October 2023 to March 2024 | S2 |
| `lte-good` | delay 24 ms | delay 8 ms | LTE, medians of the one-way delay, Vienna 2012 | S1 |
| `lte-typical` | delay 10.55 ms | delay 10.55 ms | 4G in the UK, median round trip 21.1 ms, 2025 | S3 |
| `5g-typical` | delay 9.1 ms | delay 9.1 ms | 5G in the UK, median round trip 18.2 ms, 2025 | S3 |
| `5g-poor` | delay 21.8 ms | delay 21.8 ms | the first non-standalone 5G deployments (China, 3.5 GHz, 2020), half of the round trip | S4 |
| `dsl-good` | delay 10 ms | delay 10 ms | DSL in the US, idle round trip of the fastest speed tier, 20 ms, 2022 | S5 |
| `dsl-poor` | delay 30.5 ms | delay 30.5 ms | DSL in the US, idle round trip of the slowest speed tier, 61 ms, 2022 | S5 |
| `geo-good` | delay 298.5 ms, loss 0.025 % | delay 298.5 ms, loss 0.025 %, rate 144 mbit | one Viasat-2 terminal without a proxy, round trip 597 ms, loss of the round trip 0.05 %, peak rate of the plan downstream | S6 |
| `geo-typical` | delay 336.75 ms | delay 336.75 ms | all geostationary operators in the M-Lab data, median latency 673.5 ms | S7 |
| `geo-poor` | delay 417.6 ms | delay 417.6 ms | the slowest geostationary operator in the M-Lab data, median latency 835.2 ms | S7 |
| `leo-typical` | delay 22.5 ms | delay 22.5 ms | Starlink, global median latency 40 to 50 ms, the middle | S8 |
| `datacenter-rack` | delay 108 us, loss 0.000655 % | delay 108 us, loss 0.000655 % | inside a pod of one data center, median round trip 216 us, drop rate 1.31e-5 | S9 |
| `datacenter-fabric` | delay 134 us, loss 0.003775 % | delay 134 us, loss 0.003775 % | between pods of that data center, median round trip 268 us, drop rate 7.55e-5 | S9 |

### How each value follows from its source

- **`3g-good`, `lte-good`.** The medians of the one-way delay of an HSPA and an LTE network, measured with synchronised clocks: HSPA 18 ms downlink and 15 ms uplink, LTE 8 ms downlink and 24 ms uplink ✓ [S1]. The uplink of LTE is slower than the one of HSPA, which the profile keeps. No loss, jitter or rate is given.
- **`3g-typical`.** The average "response time" of 3G connections in the UK was 42.3 ms ([S2], Opensignal data in the report of Ofcom); half is 21.15 ms. The page does not say whether it is a mean or a median. 4G was 21.9 ms and 5G 18.9 ms in the same period.
- **`lte-typical`, `5g-typical`.** The median latency across the UK in 2025 was 21.1 ms on 4G and 18.2 ms on 5G ✓ [S3]; half is 10.55 ms and 9.1 ms. The same page says standalone 5G has about 15 % lower latency than non-standalone.
- **`5g-poor`.** The first large measurement of non-standalone 5G found a network latency of 21.8 ms ("half of the round trip") on average ✓ [S4]; the same study finds the 5G paths 22.3 ms faster than 4G and says the latency is not enough for interactive applications. It is the highest published figure for 5G in the sources used.
- **`dsl-good`, `dsl-poor`.** The weighted median idle latency by advertised speed ranged from 20 ms to 61 ms for DSL ✓ [S5] (the idle latency by provider was 23 to 34 ms). Half is 10 ms and 30.5 ms. This is a round trip to the nearest test server of the study.
- **`geo-good`.** One terminal in Massachusetts, a Ka-band service over Viasat-2, with the proxy off, measured on a link without load ✓ [S6]: the round trip had a median of 597 ms (half is 298.5 ms) and a deviation of 16.9 ms, and 604 packets were lost, about 0.05 %. A packet of the round trip is lost if either way loses it, so the loss of one way is `1 - sqrt(1 - 0.0005)`, 0.025 %. The downstream plan "provides a peak data rate of 144 Mb/s" ✓ [S6], which is the rate of `down`. The study does not give the rate of the uplink, so `up` has none. The other numbers of the study (a queue of 36 MB that allows about 2 s at the peak rate, losses in bursts of up to 11 packets) are properties the profile cannot set yet.
- **`geo-typical`, `geo-poor`.** The median latency of the geostationary operators was 673.5 ms, that of the slowest (KVH) 835.2 ms ✓ [S7], the best (SSI) 620.4 ms; halves are 336.75 ms and 417.6 ms.
- **`leo-typical`.** "The median latency of Starlink hovers around 40 to 50 ms" ✓ [S8], from the M-Lab data of many countries; half of the middle, 45 ms, is 22.5 ms. The distribution differs much by region (Manila is much higher).
- **`datacenter-rack`, `datacenter-fabric`.** Inside a pod the 50th percentile of the round trip was 216 us, between pods 268 us ✓ [S9] (data center DC1); the drop rates of DC1 were 1.31e-5 inside a pod and 7.55e-5 between pods ✓. The probe is a TCP handshake whose two packets can each be lost, so the loss of one packet is taken as half, 0.000655 % and 0.003775 %. The probe includes the hosts (the time of the network stacks), so the delay is that of a path with its servers and not only of the switches. It is one data center of one operator in 2015.

## What is not in version 1

| What | What exists | Why it is not set |
| --- | --- | --- |
| Wi-Fi | One campus (47,000 devices): median round trip about 3 ms, 90th percentile about 20 ms, 99th about 250 ms; 5 GHz 35 % faster than 2.4 GHz ✓ [S10] | A long tail needs a distribution of delays; with three quantiles the shape between them would be invented. No source for the rate or the loss of a home Wi-Fi |
| Cable (DOCSIS) | Idle round trip 12 to 24 ms by provider ✓ [S5] | One key only, no tiers worth the name; rates and queues have no verified source |
| Lossy radio (LoRaWAN, 802.15.4, telemetry radios) | Data rates of the standards (LoRaWAN 250 bit/s to 5,470 bit/s, 802.15.4 250 kbit/s); the telemetry radio of ArduPilot 64 kbit/s | The rates of a standard are the maximum of a PHY, not what a link delivers; no published loss or delay of a link that is not a bound |
| Starlink: the pattern of 15 s | The satellite is changed every 15 s, at the 12th, 27th, 42nd and 57th second of a minute ✓ [S11]; for one terminal the first 140 ms and the last 75 ms of a period are affected, with 74 ms more on the uplink at the peak ✓ [S12] | The size and the shape of the spike are from one terminal and one preprint, and the loss at the switch has no source. The period is solid, the state is not; it comes with a source for the state |
| LEO tiers `good` and `poor`, OneWeb | Starlink minimum ping 20 ms and a 75th percentile above 100 ms round trip in South America (summaries of papers); OneWeb median 154 ms (M-Lab) | Read through slides or summaries only; they are added when the papers themselves were checked |
| Loss, jitter, rates and queues of the cellular tiers | Rates as shares of connections ("47 % of 5G downloads at 100 mbit or more"); a burst model fitted to LTE ✓ [S13] | A share is not a rate. The fitted model is of packets that missed a deadline of 38 ms, not of lost packets; used as loss it would be wrong |
| mmWave 5G, blockage and obstruction | Peak rates in the lab; no duration or frequency of blockage found | No source |
| Datacenter between zones and regions | Cloud providers publish medians of the round trip (they change every month) and a design target of 2 ms between zones | A monthly table of a provider is a snapshot of a provider; a design target is a bound |

When a source arrives, the values go into `crates/netscenario-model/profiles/library.yaml` with the source, this page changes with them, and the version of the library goes up. A scenario that was made with an older version keeps its numbers (D-095).

## Sources

✓ means the figure was compared with the paper or the page. "Via" means the page of a news site or a summary that quotes the report.

- **S1** ✓ Laner et al., "A Comparison Between One-way Delays in Operating HSPA and LTE Networks", 2012, https://www.eurecom.fr/publication/3694/download/cm-publi-3694.pdf. Medians of the one-way delay: HSPA 18 ms down and 15 ms up, LTE 8 ms down and 24 ms up.
- **S2** ✓ Ofcom, Mobile Matters 2024 (Opensignal data, October 2023 to March 2024), via https://www.ispreview.co.uk/index.php/2024/09/ofcom-study-benchmarks-speeds-of-uk-4g-and-5g-mobile-networks.html. Average latency 3G 42.3 ms, 4G 21.9 ms, 5G 18.9 ms.
- **S3** ✓ Ofcom, Mobile Matters 2025, via http://5g.co.uk/news/ofcom-study-shows-5g-sa-has-lot-of-potential/5570/. Median latency 4G 21.1 ms, 5G 18.2 ms; standalone 5G about 15 % lower than non-standalone.
- **S4** ✓ Xu et al., "Understanding Operational 5G: A First Measurement Study on Its Coverage, Performance and Energy Footprint", SIGCOMM 2020, https://www.cs.princeton.edu/courses/archive/spring21/cos563/papers/5G_measurement.pdf. Network latency (half of the round trip) 21.8 ms on average; 22.3 ms below 4G.
- **S5** ✓ FCC, "Measuring Fixed Broadband, Thirteenth Report" (tests of September and October 2022), https://data.fcc.gov/download/measuring-broadband-america/2023/2023-Fixed-Measuring-Broadband-America-Report.pdf. Idle latency DSL 23 to 34 ms by provider, cable 12 to 24 ms, fiber 7 to 14 ms; by speed tier DSL 20 to 61 ms, cable 13 to 22 ms.
- **S6** ✓ Claypool, Chung, Claypool, "Comparison of TCP Congestion Control Performance over a Satellite Network", PAM 2021, https://web.cs.wpi.edu/~claypool/papers/tcp-sat-pam-21/camera-draft.pdf. Median round trip 597 ms, deviation 16.9 ms, 604 packets lost (0.05 %), peak data rate 144 Mb/s, queue up to 36 MB.
- **S7** ✓ Raman et al., "Dissecting the Performance of Satellite Network Operators", arXiv 2310.15808, 2023. Median latency of geostationary operators 673.5 ms; best operator 620.4 ms, worst (KVH) 835.2 ms.
- **S8** ✓ Mohan et al., "A Multifaceted Look at Starlink Performance", WWW 2024, https://arxiv.org/abs/2310.09242. The median latency of Starlink "hovers around 40 to 50 ms".
- **S9** ✓ Guo et al., "Pingmesh: A Large-Scale System for Data Center Network Latency Measurement and Analysis", SIGCOMM 2015. Round trip 216 us / 268 us (50th percentile, intra-pod / inter-pod, DC1), 1.26 ms / 1.34 ms (99th); drop rates DC1 1.31e-5 / 7.55e-5.
- **S10** ✓ Sui et al., "Characterizing and Improving WiFi Latency in Large-Scale Operational Networks", MobiSys 2016, https://microsoft.com/en-us/research/wp-content/uploads/2017/01/mobisys_2016b.pdf. Median about 3 ms, 90th percentile about 20 ms, 99th about 250 ms (round trip, access point to device); 5 GHz 35 % lower than 2.4 GHz.
- **S11** ✓ Tanveer et al., "Making Sense of Constellations", arXiv 2307.00402, 2023. Reconfiguration every 15 s, at the 12th, 27th, 42nd and 57th second of a minute.
- **S12** ✓ Casparsen et al., "Statistical Characterization and Prediction of E2E Latency over LEO Satellite Networks", arXiv 2601.08439, January 2026 (a preprint, one terminal). The first 140 ms and the last 75 ms of each 15 s period are affected; 74 ms above the mean at the start.
- **S13** ✓ Nielsen, Leyva-Mayorga, Popovski, arXiv 1909.03875, 2019. Uplink delay of LTE: 90 % of the packets within about 36 ms, 99 % within 40 ms; a Gilbert-Elliott fit with p = 0.0178516 and r = 0.257756 for packets that missed a deadline; no real loss.

## Changelog of the library

- **Version 1** (2026-11-10): 14 profiles of 3G, LTE, 5G, DSL, GEO, LEO and datacenter, delays and the loss and rate of two of them.
