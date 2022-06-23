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
	"bytes"
	"testing"

	"github.com/stretchr/testify/require"
)

func TestOSReleaseID(t *testing.T) {
	for _, tc := range []struct {
		in       string
		expected string
		errfn    func(t require.TestingT, err error, msgAndArgs ...interface{})
	}{
		{
			in: `
NAME="Amazon Linux"
VERSION="2"
ID="amzn"
ID_LIKE="centos rhel fedora"
VERSION_ID="2"
PRETTY_NAME="Amazon Linux 2"
ANSI_COLOR="0;33"
CPE_NAME="cpe:2.3:o:amazon:amazon_linux:2"
HOME_URL="https://amazonlinux.com/"
`,
			expected: "amzn",
			errfn:    require.NoError,
		},
		{
			in: `
PRETTY_NAME="Debian GNU/Linux bookworm/sid"
NAME="Debian GNU/Linux"
ID=debian
HOME_URL="https://www.debian.org/"
SUPPORT_URL="https://www.debian.org/support"
BUG_REPORT_URL="https://bugs.debian.org/"
`,
			expected: "debian",
			errfn:    require.NoError,
		},
		{
			in: `
PRETTY_NAME="Debian GNU/Linux bookworm/sid"
NAME="Debian GNU/Linux"
HOME_URL="https://www.debian.org/"
SUPPORT_URL="https://www.debian.org/support"
BUG_REPORT_URL="https://bugs.debian.org/"
`,
			errfn: require.Error,
		},
	} {
		reader := bytes.NewReader([]byte(tc.in))
		got, err := OSReleaseID(reader)
		tc.errfn(t, err)
		require.Equal(t, tc.expected, got)
	}
}
