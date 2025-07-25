#!/bin/bash

echo "Fact unified agent demo"
echo "======================"
echo
echo "Usage examples:"
echo
echo "1. File monitor mode (default):"
echo "   fact --paths /tmp:/var/log --url https://sensor.example.com"
echo
echo "2. VM agent mode:"
echo "   fact --mode vm-agent --url https://sensor.example.com --interval 3600"
echo
echo "3. VM agent with VSOCK:"
echo "   fact --mode vm-agent --use-vsock"
echo
echo "4. Dry run mode:"
echo "   fact --mode vm-agent --skip-http"
echo
echo "Configuration via environment:"
echo "   FACT_MODE=vm-agent"
echo "   FACT_URL=https://sensor.example.com"
echo "   FACT_RPMDB=/host/var/lib/rpm"
echo "   FACT_HOST_MOUNT=/host"
echo
echo "Docker usage:"
echo "   # File monitor (privileged K8s node agent)"
echo "   docker run --privileged fact:latest --paths /var/log"
echo "   "
echo "   # VM agent"
echo "   docker run -v /var/lib/rpm:/host/var/lib/rpm fact:latest --mode vm-agent"