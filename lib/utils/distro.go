/*
Copyright 2022 Gravitational, Inc.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package utils

import (
	"bufio"
	"errors"
	"io"
	"strings"

	"strconv"

	"github.com/gravitational/trace"
)

// Linux distros with official packages mentioned in `Installing Teleport` docs:
//
// Amazon Linux 2:
//   /etc/os-release:
//      NAME="Amazon Linux"
//      VERSION="2"
//      ID="amzn"
//      ID_LIKE="centos rhel fedora"
//      VERSION_ID="2"
//      PRETTY_NAME="Amazon Linux 2"
//      ANSI_COLOR="0;33"
//      CPE_NAME="cpe:2.3:o:amazon:amazon_linux:2"
//      HOME_URL="https://amazonlinux.com/"
//
//   /etc/system-release: Amazon Linux release 2 (Karoo)
//
// RHEL:
//   /etc/os-release
//      NAME="Red Hat Enterprise Linux"
//      VERSION="8.5 (Ootpa)"
//      ID="rhel"
//      ID_LIKE="fedora"
//      VERSION_ID="8.5"
//      PLATFORM_ID="platform:el8"
//      PRETTY_NAME="Red Hat Enterprise Linux 8.5 (Ootpa)"
//      ANSI_COLOR="0;31"
//      CPE_NAME="cpe:/o:redhat:enterprise_linux:8::baseos"
//      HOME_URL="https://www.redhat.com/"
//      DOCUMENTATION_URL="https://access.redhat.com/documentation/red_hat_enterprise_linux/8/"
//      BUG_REPORT_URL="https://bugzilla.redhat.com/"
//
//      REDHAT_BUGZILLA_PRODUCT="Red Hat Enterprise Linux 8"
//      REDHAT_BUGZILLA_PRODUCT_VERSION=8.5
//      REDHAT_SUPPORT_PRODUCT="Red Hat Enterprise Linux"
//      REDHAT_SUPPORT_PRODUCT_VERSION="8.5"

// Debian
//   /etc/os-release
//      PRETTY_NAME="Debian GNU/Linux bookworm/sid"
//      NAME="Debian GNU/Linux"
//      ID=debian
//      HOME_URL="https://www.debian.org/"
//      SUPPORT_URL="https://www.debian.org/support"
//      BUG_REPORT_URL="https://bugs.debian.org/"
//
// Ubuntu
//   /etc/os-release
//      NAME="Ubuntu"
//      VERSION_ID="22.04"
//      VERSION="22.04 (Jammy Jellyfish)"
//      VERSION_CODENAME=jammy
//      ID=ubuntu
//      ID_LIKE=debian
//      HOME_URL="https://www.ubuntu.com/"
//      SUPPORT_URL="https://help.ubuntu.com/"
//      BUG_REPORT_URL="https://bugs.launchpad.net/ubuntu/"
//      PRIVACY_POLICY_URL="https://www.ubuntu.com/legal/terms-and-policies/privacy-policy"
//      UBUNTU_CODENAME=jammy

var ErrOSReleaseIDNotFound = errors.New("field ID not found in os-release file")

func OSReleaseID(rel io.Reader) (string, error) {
	scanner := bufio.NewScanner(rel)
	for scanner.Scan() {
		kvLine := scanner.Text()
		if kvLine == "" || kvLine[0] == '#' {
			continue
		}
		key, value, found := strings.Cut(kvLine, "=")
		if !found {
			continue
		}
		// only interested in the ID field
		if key != "ID" {
			continue
		}
		// unquote value if it is quoted (rhel, amzl)
		if value[0] == '"' {
			var err error
			value, err = strconv.Unquote(value)
			if err != nil {
				return "", trace.WrapWithMessage(err, "invalid ID field")
			}
		}
		return value, nil
	}
	return "", ErrOSReleaseIDNotFound
}
