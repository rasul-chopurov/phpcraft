#!/bin/sh
# Builds the site and uploads site/public to path.az over FTP (lftp); nothing on the server is deleted.
# The password is never stored: pass it in PATHAZ_FTP_PASSWORD or type it at the prompt.
#   ./site/deploy.sh
set -e
cd "$(dirname "$0")"
HOST=${PATHAZ_FTP_HOST:-109.205.182.167}
USER=${PATHAZ_FTP_USER:-path_az}
REMOTE=${PATHAZ_FTP_DIR:-/var/www/path_az_usr13/data/www/path.az}

python3 -I build.py
if [ -z "$PATHAZ_FTP_PASSWORD" ]; then
  printf "FTP password for %s@%s: " "$USER" "$HOST"
  stty -echo; read -r PATHAZ_FTP_PASSWORD; stty echo; echo
fi
export LFTP_PASSWORD="$PATHAZ_FTP_PASSWORD"
lftp --env-password -u "$USER" "$HOST" <<EOF
set ftp:ssl-allow yes
set ssl:verify-certificate no
set net:max-retries 2
set net:timeout 20
cd "$REMOTE" || cd www/path.az || exit 1
mirror --reverse --verbose --exclude-glob .DS_Store public/ ./
bye
EOF
echo "Deployed to https://path.az/"
