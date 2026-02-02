AegeanSymphonicOrchestra-v2_5-universal.sf2: aegean.zip
	unzip -u $< license.txt $@

aegean.zip:
	wget --no-check-certificate \
		"https://drive.usercontent.google.com/download?id=17z0U-463nsXEwadrWycBUmZKslugT_tN&confirm=t" \
		-O "$@"
	sha256sum -c checksum.sha256

.DELETE_ON_ERROR: aegean.zip
