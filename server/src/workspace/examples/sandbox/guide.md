# The Sandbox

A place to try things. Cards here are not tied to a project and stay out of the **All** view, so nothing you make in the Sandbox gets in the way of real work.

## What to do here

- **Write a document.** Add a step, choose a Markdown file and edit it in the card. Markdown saves keep the last versions, so an experiment can be undone.
- **Try a report.** Copy `playground.html` (the next tab), change it, and see how a sandboxed report looks and behaves. Link `../_shared/report.css` and `../_shared/report.js` for the standard look.
- **Draw a dashboard.** The `Dashboard` tab is a report with charts and live data. It uses `../_shared/chart.umd.js` (Chart.js) and `../_shared/report-charts.js` for the look, and `workbench.query` to read your project's sessions, git state, CI runs and data-viewer series. In the Sandbox it shows sample data; in a card of a project it asks you once to **Allow** it.
- **Let an agent experiment.** Ask a session to make a card in the scope `wb-sandbox`, for example *"Draft a status report as a card in the Workbench sandbox."* Agents in any project may write here.
- **Test a viewer.** Drop images, a PDF, a `.glb` model or a folder of screenshots into a card and see which viewer takes them.

## Resetting

**Reset** in the Sandbox's toolbar moves every card to the Workspace trash and brings this guide back. Nothing is deleted for good: open **Trash** to restore a card or empty it.

## What a report cannot do

Reports run in a browser sandbox with no access to Workbench, its cookies or its storage, and no network beyond their own files. `playground.html` checks this live.

The one way out is `workbench.query`: a report asks the Workbench page around it for a few kinds of project data, read-only, and only after you allowed that card (the strip above the report, and the pulse icon in its toolbar to stop). Cards from a repository, and cards in Home or the Sandbox, never get any. A report you allow can send what it reads to other sites, so allow only reports you trust.
