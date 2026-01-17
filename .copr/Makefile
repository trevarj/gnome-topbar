srpm:
	dnf -y install rpm-build rpmdevtools cargo rust
	# Download source tarball
	spectool -g -C . vibepanel.spec
	# Extract, vendor dependencies, and repack
	tar xzf vibepanel-*.tar.gz
	cd vibepanel-*/ && \
		mkdir -p .cargo && \
		cargo vendor > .cargo/config.toml
	tar czf vibepanel-*.tar.gz vibepanel-*/
	rm -rf vibepanel-*/
	# Build SRPM
	rpmbuild -bs vibepanel.spec --define "_sourcedir $(PWD)" --define "_srcrpmdir $(outdir)"
