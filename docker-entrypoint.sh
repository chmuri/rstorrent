#!/bin/sh
set -e

# If the configuration file does not exist in the volume, copy the default template
if [ ! -f /config/.rstorrent.rc ] && [ ! -f /config/.rtorrent.rc ]; then
    echo "Initializing /config/.rstorrent.rc from default template..."
    mkdir -p /config
    cp /etc/rstorrent/rstorrent.default.rc /config/.rstorrent.rc
    ln -sf /config/.rstorrent.rc /config/.rtorrent.rc
elif [ -f /config/.rtorrent.rc ] && [ ! -f /config/.rstorrent.rc ]; then
    ln -sf /config/.rtorrent.rc /config/.rstorrent.rc
elif [ -f /config/.rstorrent.rc ] && [ ! -f /config/.rtorrent.rc ]; then
    ln -sf /config/.rstorrent.rc /config/.rtorrent.rc
fi

# Ensure required directories exist
mkdir -p /downloads /session /config

exec "$@"
