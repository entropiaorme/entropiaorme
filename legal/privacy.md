# Privacy Policy

Version 1.3. Effective 2026-11-19.

EntropiaOrme processes your data on your own computer. It has no telemetry, no analytics, no remote logging, and no account system of its own, and it sends nothing to the developer. The one connection it can make is to your own Entropia Universe account, and only if you choose to connect it. See "Network access" below.

## What the app processes locally

To provide its analytical features, the app reads and processes the following on your machine:

Entropia Universe game logs. The app parses log files written by the Entropia Universe client to your local filesystem, and reads them only to extract game events relevant to its tracking features.

On-screen content via OCR. When OCR-driven features are enabled, the app captures specific regions of your screen and runs optical character recognition locally to extract text from the Entropia Universe client's interface, such as the Skills and Professions panels, the Repair and Trade Terminal windows, the radar, creature names, and the tool in hand.

Input events. When input-listening features (such as hotbar tracking) are enabled, the app observes keyboard and mouse events while those features are active. The scope of listening is limited to what each feature requires.

Your Entropia Universe account readings. If you connect your account (see "Network access"), the app reads your item list and your PED Card balance from the account site and stores those readings locally.

Configuration and tracked data you enter. Settings, equipment loadouts, quest playlists, and similar inputs you provide to the app.

This processed data is used only to populate the app's own analytical surfaces (sessions, character, equipment, quests, analytics) on your machine.

## What the app does not do

It does not transmit any of this data, screen captures, OCR output, input events, configuration, or anything else to the developer.

It does not contact any remote analytics, telemetry, crash-reporting, or advertising service.

It does not require an account, login, or any form of personal identification. Connecting your Entropia Universe account is optional.

It does not collect or process information about other users or persons; the app is single-user and operates only on the data of the person running it.

## Where data is stored

All data is stored on your local machine, in locations including:

Application settings: {your user data directory}/EntropiaOrme/settings.json (managed via the Tauri store).

Tracked data: a local SQLite database within the app's application data directory.

Account sign-in: if you connect your account, the session the account site issues is kept in your operating system's credential store (the Windows Credential Manager, or the desktop keyring on Linux), never in a plain file. Disconnecting your account in the app removes it.

You can delete this data at any time by removing the app's application data directory using your operating system's normal file-management tools.

## Network access

The app makes outbound network requests in one circumstance only.

Your Entropia Universe account. If you choose to connect your account, you sign in on the Entropia Universe account site itself, in a window the app opens; the app never sees your password. While connected, the app asks that site for your item list and your PED Card balance, about once a minute, using the session the site issued to you. These requests go only to the Entropia Universe account site, which is operated by MindArk, and your use of that site is governed by its own terms and privacy policy. Nothing is sent to the developer or to anyone else.

If you do not connect your account, the app makes no network requests.

## Third parties

The app does not share data with any third party; the one connection it can make, to your own Entropia Universe account, is described under "Network access". It does, however, link out to the Entropia Nexus website (https://entropianexus.com/) as a reference source for game data; following such a link is governed by that third party's own terms and policies, not by this Policy.

## Children's privacy

The app is not directed at children. Because no personal data is collected by or transmitted to the developer, no special procedures for children's data apply.

## Changes to this policy

The developer may update this Privacy Policy from time to time. Material changes will be reflected by an increment to the version above. The in-app acceptance flow that surfaces the Terms of Service will also surface a re-acceptance prompt for the Privacy Policy in such cases.

## Governing law

This Privacy Policy is governed by and construed in accordance with the laws of England and Wales.

## Contact

Questions about this Privacy Policy can be directed to MikelWL@protonmail.com.
