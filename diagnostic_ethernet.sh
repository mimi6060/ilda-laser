#/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

echo "=================================="
echo " DIAGNOSTIC ETHERNET - MAC"
echo "=================================="

for i in en4 en5 en6; do
    echo
    echo "===== Interface $i ====="
    ifconfig "$i" | grep -E 'flags=|status:|inet '
done

echo
echo "===== Adresses IP ====="
ipconfig getifaddr en4 2>/dev/null || true
ipconfig getifaddr en5 2>/dev/null || true
ipconfig getifaddr en6 2>/dev/null || true

echo
echo "===== Matériel Ethernet ====="
networksetup -listallhardwareports

echo
echo "===== Fin du diagnostic ====="
