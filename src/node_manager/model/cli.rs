/*
   Copyright 2021 JFrog Ltd

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

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]

pub struct Status {
    pub peers_count: usize,
    pub incomplete_peer_count: bool,
    pub artifact_count: usize,
    pub disk_allocated: usize,
    pub disk_usage: f64,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Connected Peers Count:       {}", self.peers_count)?;
        if self.incomplete_peer_count {
            write!(
                f,
                " (Peer counting ran out of time before it could count all peers)"
            )?;
        }
        writeln!(f)?;
        writeln!(f, "Artifacts Count:             {}", self.artifact_count)?;
        writeln!(f, "Total Disk Space Allocated:  {}", self.disk_allocated)?;
        write!(f, "Disk Space Used:             {}%", self.disk_usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn fmt_test() -> std::fmt::Result {
        let status = Status {
            peers_count: 2,
            incomplete_peer_count: true,
            artifact_count: 4,
            disk_allocated: 1000000,
            disk_usage: 0.34f64,
        };
        let formatted = format!("{}", status);
        assert_eq!("Connected Peers Count:       2 (Peer counting ran out of time before it could count all peers)\nArtifacts Count:             4\nTotal Disk Space Allocated:  1000000\nDisk Space Used:             0.34%",
                   formatted);
        Ok(())
    }
}
