# Avatars

A session can have a **live avatar**: a small animated character in the workspace window that the session draws itself. It says the short version of what the session does, aloud or as captions, asks you questions and takes your answer, and a glow around it shows the session's state in the colours of its tab. The detail stays in the terminal.

## Who has an avatar

**Settings › Avatar** decides which sessions may have one (`[avatar]` in `config.toml`): **Whenever they want** (the default: a session gets one the first time it uses one, and may start another session with one), **When I give them one** (sessions that have one already keep it until you turn it off for each) or **Off**. You can turn any session's avatar off, and the session never turns it on again.

To give a session one yourself, tick **Avatar** among the options of the new-session composer (after **Ultracode**; every CLI has it). For a session that already runs, use **Ctrl+K** → *Avatar: Give This Session an Avatar*, or the avatar widget's menu in the status bar. The session is told, draws its look, name and voice in two or three calls, looking at a picture of what it drew in between (about half a minute and a few thousand tokens), and greets you. Its tab and card show an **Avatar** badge.

## What you see

In the desktop layout the figures walk about the middle of the workspace window, never over the agents column or the tool windows. They rest where nothing you press or type into is under them; where there is no such place (an editor fills the window) they stand at home, in their column or where you dropped them, and visit only each other there. Their home follows the column when a tool window opens or closes. Now and then one visits another and they greet each other; a figure whose session works neither visits nor is visited. Only the character is seen, with no box around it. A session may tie parts of its figure to where it looks, how it walks and your dragging: a head that turns to the pointer, legs that dangle while you carry it. A figure stands still while it speaks, asks or plays a gesture (it turns to you for that), and when the moving pointer comes near it, unless it stands on a button, a tab, an editor or a terminal: there it walks on. Under the pointer it stays small and shows its name, and it grows when its bubble opens; two open bubbles never cover each other. Drag a figure to put it elsewhere: it keeps that place while it stays on screen and walking stays on. With **Walk around** off (Settings or the figure's menu), or with **Reduce motion**, the figures stand in a column instead (top right until you move it), up to four.

A figure's bubble shows the line as a caption, under Workbench's own header (the session's title, then the avatar's name in quotation marks). Click the figure to read its last lines and to write to the session; **Esc** closes the bubble. Hover it for **Reply**, **Open the session** and the menu. With the workspace window collapsed, and on a phone, the avatar is a face in the top bar and its bubble a panel under it. Another project's avatar appears only while it asks you something.

## Hearing it

Captions always work. Voice is off until you choose it in this browser: press **Hear it** under a line, or **Captions only**. A browser that has only online voices asks first with **Allow online voices**: what an avatar says is then sent to the browser maker's speech service. Only one tab of all your devices says a line aloud. **Stop** in the bubble, or a click on the figure, stops it. After a reload, **Click to hear** lets the browser speak again.

## Answering

Click a choice, or type in the reply box and press **Enter**. The microphone in the box takes your answer by voice (your browser's speech recognition hears it, not Workbench); nothing is sent until you press **Enter**. **Dismiss** closes a question without an answer. When the session has stopped waiting, your answer is typed into the session as a prompt that starts with `[via avatar, …]`; if a dialog opens meanwhile, it waits in the session's input box: answer the dialog, then press **Enter** there. The reply box also sends a message to the session when there is no question.

When the session needs a permission, the avatar steps back: it shows **Waiting for your decision in the session** with **Open**. You decide in the session, never through the avatar.

## Hiding and turning off

The figure's menu has **Hide all avatars** (nothing is shown or said in this browser until **Show avatars** in the status bar's menu or Settings), **Turn off for this session** (on every device), **Use the default look**, **Move to…** and **Reset position** (while the figures stand in their column), **Walk around** and **Reduce motion**. **Settings › Avatar › On this device** has voice, online voices, **Test voice**, speed, volume, walking and motion; the phone's **More** tab has **Show avatars** and **Voice**.

## Limits

A line is at most 400 characters and a question has up to four choices. Kimi Code, Gemini CLI, Aider and custom CLIs drive their avatar with the `workbench avatar` command, and in a dev container they and Codex use `curl`; their CLI may ask you to approve it. Not yet tried: sound through a real loudspeaker, dictation with a real microphone, real agent CLIs (only the tests' stand-ins were used), a real model looking at the pictures of its avatar, a session in a dev container, Windows at run time, Safari and iOS.

More in the repository's `docs/avatar.md`.
