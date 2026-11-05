# Gantry

Gantry is a self-hosted dashboard for monitoring and controlling one person's or one small workshop's 3D printers from a single place.

## Fleet

**Fleet**:
Every Printer that one Gantry deployment manages. A deployment has exactly one Fleet.
_Avoid_: Farm, tenant, account

**Printer**:
One physical machine configured in Gantry, with its own name, connection details and capabilities.
_Avoid_: Device, machine, node

**Driver**:
The adapter that speaks one printer protocol (OctoPrint, Moonraker, Bambu LAN, …). One Driver serves many Printers.
_Avoid_: Plugin, integration, connector, backend

**Simulated Driver**:
A Driver that imitates a Printer in software, with controllable behaviour (speed, Faults, dropouts). It ships with Gantry and is chosen like any other Driver.
_Avoid_: Mock driver, fake printer (in this codebase "mock" is reserved for test doubles)

**Tag**:
A label an Admin attaches to Printers for filtering, such as a room or a purpose. A Printer can have many Tags.
_Avoid_: Group, folder, category

**Filament Slot**:
One position in a Printer's filament supply, holding one filament with a type, colour and amount remaining.
_Avoid_: AMS slot, MMU slot, tray, lane

**Fault**:
A problem a Printer reports about itself, such as a clog, runout or open door, with a severity of warning or error.
_Avoid_: HMS error, alarm, alert (an Alert is the notification Gantry sends, not the problem itself)

**Capability**:
Something a Printer can do, such as pause, a camera or a work light, as its Driver reports it. The UI offers only what a Printer has. An Admin can override a capability when the Driver can't detect it, and firmware that refuses local control withdraws every control capability while the Printer stays connected.
_Avoid_: Feature, permission (a permission is what a user's role allows)

**Fine control**:
A control beyond pause, resume and Cancel: speed profile, temperatures, lights, fans, homing, jogging, or loading and unloading filament. Each has its own Capability.
_Avoid_: Advanced control, manual control

## Status

**Connection**:
Whether Gantry can currently talk to a Printer: connected, connecting or disconnected, with a reason such as unreachable or authentication rejected. Firmware blocking control is not a Connection problem; it removes control capabilities from a connected Printer.
_Avoid_: Online/offline (as a Printer State)

**Printer State**:
What a connected Printer is doing. It is unknown while the Printer is disconnected.
_Avoid_: Status (too broad: status covers Connection, Printer State, temperatures and more)

**Preparing**:
The Printer State after a Job starts but before it is printing, such as while heating or levelling.
_Avoid_: Starting, warming up

**Busy**:
The Printer State while a Printer does something that isn't a Job, such as homing, loading filament or calibrating.
_Avoid_: Working, error

**Awaiting Clear**:
The Printer State after any Job ends, while the finished part may still be on the bed. The Queue never assigns a Job to a Printer in this state.
_Avoid_: Finished, done (those describe a Job, not a Printer)

**Mark Clear**:
An Operator confirming that a Printer's bed is empty, which takes it out of Awaiting Clear.
_Avoid_: Reset, acknowledge

**Auto-Clear**:
A per-Printer setting for machines that eject parts themselves. After a Succeeded Job the Printer goes straight to Idle instead of Awaiting Clear.
_Avoid_: Auto-eject (that is the printer's feature, not Gantry's setting)

**Auto-start**:
A per-Printer setting, on by default, that lets the Queue start the next compatible Job as soon as the Printer becomes Idle.
_Avoid_: Auto-dispatch, auto-print

## Printing

**Print File**:
A sliced file held in the library (`.gcode.3mf` or `.gcode`), ready to print. It was sliced for a specific printer model.
_Avoid_: Model, STL, gcode (as a noun for the file)

**Plate**:
One printable layout inside a Print File, with its own thumbnail and gcode. A plain `.gcode` Print File has exactly one Plate.
_Avoid_: Build plate (the physical bed), page, part

**Job**:
One attempt to print one Plate on one Printer. A Job started outside Gantry is still a Job, but its Print File may be unknown.
_Avoid_: Print, task, run

**External Job**:
A Job that started without Gantry, for example from the printer's own screen or a vendor's app. Gantry records it as soon as it sees it, and its Print File may be unknown.
_Avoid_: Foreign job, unmanaged print, manual print

**Job Outcome**:
How a Job ended: Succeeded, Failed, Cancelled or Lost (Gantry never saw it end). An Operator may overrule Succeeded to Failed.
_Avoid_: Result, status

**Timelapse**:
Frames taken from a Printer's camera during one Job (one per layer, or on a timer) and kept with that Job in History. Off unless an Admin turns it on for the Printer.
_Avoid_: Recording, video

**Queue**:
The Jobs waiting to be assigned to a Printer.
_Avoid_: Backlog, schedule

**Compatible Printer**:
A Printer whose model matches the model a Print File was sliced for, and whose Filament Slots hold the filament types the Print File needs.
_Avoid_: Free printer, available printer

**Cancel**:
Ending a Printer's current Job cleanly, at the user's request.
_Avoid_: Stop, abort, kill

**Alert**:
A notification Gantry sends through an outside channel (Discord, ntfy, email, …) when something happens, such as a Job ending, a Fault or a lost Connection.
_Avoid_: Notification (as a noun for the event), alarm, Fault

**Alert channel**:
Somewhere Alerts go, such as an ntfy topic or a Discord webhook, set up by an Admin. Its credentials are encrypted at rest.
_Avoid_: Integration, destination, notifier

**Alert Rule**:
A Fleet-wide setting from an Admin that says which events, on which Printers or Tags, send an Alert to which channel.
_Avoid_: Subscription, notification setting

**Webhook**:
A URL an Admin registers to be sent the same events as Alerts, as signed JSON, for other tools to act on.
_Avoid_: Callback, hook (alone), Alert channel (a webhook isn't one; a Discord webhook URL is how one Alert channel is reached)

## People

**Viewer**:
A user who can see the whole Fleet but change nothing.
_Avoid_: Guest, read-only user

**Operator**:
A user who can also control Printers and manage Jobs and the Queue.
_Avoid_: Controller, user (too generic)

**Admin**:
A user who can also manage Printers, Tags, users and settings.
_Avoid_: Owner, superuser

**Audit log**:
The record of every state-changing action in Gantry: who did what, to which Printer, when, and how it went. Logins (successful and failed) and lockouts are in it; reads never are, and neither is any credential.
_Avoid_: Activity feed, history (History is about Jobs)

**API token**:
A secret a user makes so another tool can use Gantry's REST API as them, with their role. What it changes is in the Audit log under its name.
_Avoid_: API key (an API key is what OctoPrint and PrusaLink give Gantry), password

**Bulk action**:
One command (pause, resume or Cancel) sent to several Printers at once, as an independent command to each, with each Printer's own result: done, rejected or skipped.
_Avoid_: Batch command, broadcast
