# Data viewer

The **Data** tool window (bottom, or **Ctrl+K** → *Show Data*) and its plots show measurements while an agent captures them: a logic analyzer, a differential probe or scope, a power profiler, anything that is a value, a level or a note over time. The agent puts samples in and makes the plots; they open next to its session and fill live.

Nothing here talks to hardware by itself: the agent runs your capture tool and hands Workbench the numbers. Ask for it in plain words: *"record the load current while the board wakes up and plot it with the SPI lines"*.

## Series and plots

A **series** is one named signal: **analog** (numbers: a current, a probe waveform), **digital** (levels; a value above 1 is a bus) or **events** (text at a time: a decoded byte, a trigger). A **plot** is **lanes** under one time axis, each with its own vertical axis, like a scope or a logic analyzer: current in µA on a logarithmic lane, a rail voltage on another, each digital line on a thin one, events as a row of labels. Values get SI prefixes for their unit (5.2 mA), times the unit that suits the scale (2 ms).

Select series in the Data window and press **Plot**, or double-click one. The lane's **⋯** sets a logarithmic scale or a fixed range; **+** in the plot's bar adds a series; clicking a name in the legend hides it.

## In a plot

- **Wheel** zooms around the pointer; **drag** pans (the plot stops following); **double-click** or **End** follows the newest samples again; **F** fits everything captured.
- **Shift + drag** measures a range: min, max, mean, RMS, peak-to-peak and the area under each analog trace (charge for amps, energy for watts); frequency, duty and edge count for digital lines. **Esc** clears it.
- The chain button **links** plots of the project so they pan and zoom together: put a power plot and a logic plot side by side and they stay aligned.
- **Save as CSV** exports what is in view.

## Files the capture tool writes

**Follow file…** reads a CSV, JSON lines or VCD file inside the project while a capture tool is still writing it (and imports a finished one). A header such as `Current [uA]` gives the unit; a column called `time` is the time. An agent does the same with its `data_follow` tool, and can read statistics back (`data_query`): the sleep current, the width of a pulse.

## Limits

Samples are kept in memory (the newest million per series by default) and are not kept across a restart; the series' names and units and the plots are. Times must not go backwards within a series: a new capture starts with **Clear**. Desktop only for now.

More in the repository's `docs/data-viewer.md`.
