# OctoPrint with its virtual printer, for testing Gantry's OctoPrint Driver
# locally (PRD §5.6). The settings are baked in, and Docker copies them into the
# container's data volume when it is first created, so OctoPrint can rewrite
# them without touching the repository.
FROM octoprint/octoprint:1.11.8
COPY config.yaml users.yaml /octoprint/octoprint/
