#!/bin/bash
# Fetch Xovis geometries for a site and save to config file
#
# Usage: ./scripts/fetch-geometries.sh <site>
# Example: ./scripts/fetch-geometries.sh netto

set -e

SITE=${1:-netto}

# Site configuration
case $SITE in
  netto)
    SSH_HOST="avero@100.80.187.3"
    XOVIS_IP="10.120.48.6"
    ;;
  avero)
    SSH_HOST="avero@100.65.110.63"
    XOVIS_IP="192.168.0.XXX"  # TODO: Update with actual Avero HQ Xovis IP
    ;;
  *)
    echo "Unknown site: $SITE"
    echo "Available sites: netto, avero"
    exit 1
    ;;
esac

OUTPUT_FILE="config/geometries/${SITE}.json"
TEMP_FILE=$(mktemp)

echo "Fetching geometries for site: $SITE"
echo "SSH Host: $SSH_HOST"
echo "Xovis IP: $XOVIS_IP"

# Fetch geometries via SSH
ssh $SSH_HOST "curl -s -X GET \"http://${XOVIS_IP}/api/v5/multisensors/1/scene/geometries\" \
  -H \"accept: application/json\" \
  -H \"X-Requested-With: XmlHttpRequest\" \
  -H \"Authorization: Basic YWRtaW46QXZlcm8uLlFlZDI1MjU=\"" > "$TEMP_FILE"

# Check if we got valid JSON
if ! jq empty "$TEMP_FILE" 2>/dev/null; then
  echo "Error: Invalid JSON response"
  cat "$TEMP_FILE"
  rm "$TEMP_FILE"
  exit 1
fi

# Wrap with metadata
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
jq --arg site "$SITE" \
   --arg ts "$TIMESTAMP" \
   --arg src "http://${XOVIS_IP}/api/v5/multisensors/1/scene/geometries" \
   '{
     site: $site,
     fetched_at: $ts,
     source: $src,
     geometries: .geometries,
     total_vertices: .total_vertices,
     total_perimeter: .total_perimeter
   }' "$TEMP_FILE" > "$OUTPUT_FILE"

rm "$TEMP_FILE"

echo "Saved to: $OUTPUT_FILE"
echo ""
echo "Geometry summary:"
jq -r '.geometries[] | "\(.id) - \(.name) (\(.type))"' "$OUTPUT_FILE"
