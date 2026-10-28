//! The hosts `gh` is signed in to, per device. Pull requests on these hosts
//! open in Zeron's own view; the engine refuses every other host.
use crate::{pull_requests::params_for_target, state::EngineHandle};
use gpui::{App, Global, Task};
use std::collections::HashMap;
use zeron_rpc::methods;

const DEFAULT_HOST: &str = "github.com";

/// Signed-in hosts by device (`None` is the local device).
#[derive(Default)]
pub(crate) struct PullRequestHosts {
    by_device: HashMap<Option<String>, Vec<String>>,
}

impl Global for PullRequestHosts {}

impl PullRequestHosts {
    /// The device's hosts, or just `github.com` until they are known.
    pub(crate) fn hosts(cx: &App, device: Option<&str>) -> Vec<String> {
        cx.try_global::<Self>()
            .and_then(|hosts| hosts.by_device.get(&device.map(str::to_owned)))
            .cloned()
            .unwrap_or_else(|| vec![DEFAULT_HOST.to_owned()])
    }

    pub(crate) fn set(cx: &mut App, device: Option<String>, hosts: Vec<String>) {
        let hosts = hosts.into_iter().map(|host| host.to_lowercase()).collect();
        cx.default_global::<Self>().by_device.insert(device, hosts);
    }

    /// Ask the device for its hosts. A failed or unreadable reply keeps what
    /// was known before.
    pub(crate) fn fetch(engine: EngineHandle, device: Option<String>, cx: &mut App) -> Task<()> {
        cx.spawn(async move |cx| {
            let params = params_for_target(device.as_deref());
            let Ok(reply) = engine
                .client()
                .call(methods::LIST_CHANGE_REQUEST_HOSTS, params)
                .await
            else {
                return;
            };
            let Ok(hosts) = serde_json::from_value::<Vec<String>>(reply) else {
                return;
            };
            cx.update(|cx| Self::set(cx, device, hosts));
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use zeron_rpc::{RpcError, RpcReply, methods};

    #[gpui::test]
    fn unknown_device_defaults_to_github(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(PullRequestHosts::hosts(cx, None), ["github.com"]);
            assert_eq!(PullRequestHosts::hosts(cx, Some("remote")), ["github.com"]);
        });
    }

    #[gpui::test]
    fn hosts_are_kept_per_device(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            PullRequestHosts::set(cx, None, vec!["Acme.ghe.com".into()]);
            assert_eq!(PullRequestHosts::hosts(cx, None), ["acme.ghe.com"]);
            assert_eq!(PullRequestHosts::hosts(cx, Some("remote")), ["github.com"]);
        });
    }

    #[gpui::test]
    fn fetch_stores_hosts_for_the_targeted_device(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rpc = ScriptedRpc::new(|method, params| async move {
            assert_eq!(method, methods::LIST_CHANGE_REQUEST_HOSTS);
            assert_eq!(params["targetDeviceId"], "remote");
            RpcReply::value(&["acme.ghe.com", "github.com"])
        });
        let engine = crate::state::EngineHandle::from_test_client(rpc.client());
        let task = cx.update(|cx| PullRequestHosts::fetch(engine, Some("remote".into()), cx));
        rpc.settle(cx, &runtime, |cx| {
            cx.update(|cx| PullRequestHosts::hosts(cx, Some("remote")).len() == 2)
        });
        drop(task);
        cx.update(|cx| assert_eq!(PullRequestHosts::hosts(cx, None), ["github.com"]));
    }

    #[gpui::test]
    fn failed_fetch_keeps_the_previous_hosts(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rpc = ScriptedRpc::new(|_, _| async move { Err(RpcError::Failed("offline".into())) });
        let engine = crate::state::EngineHandle::from_test_client(rpc.client());
        cx.update(|cx| PullRequestHosts::set(cx, None, vec!["acme.ghe.com".into()]));
        let task = cx.update(|cx| PullRequestHosts::fetch(engine, None, cx));
        rpc.settle(cx, &runtime, |_| rpc.completed() == 1);
        cx.run_until_parked();
        drop(task);
        cx.update(|cx| assert_eq!(PullRequestHosts::hosts(cx, None), ["acme.ghe.com"]));
    }

    #[gpui::test]
    fn fetch_replaces_the_known_hosts_with_the_engines_list(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let rpc = ScriptedRpc::new(|method, _| async move {
            fixture::hosts_reply(&method).ok_or(RpcError::UnknownMethod(method))
        });
        let engine = crate::state::EngineHandle::from_test_client(rpc.client());
        cx.update(|cx| PullRequestHosts::set(cx, None, vec!["acme.ghe.com".into()]));
        let task = cx.update(|cx| PullRequestHosts::fetch(engine, None, cx));
        rpc.settle(cx, &runtime, |cx| {
            cx.update(|cx| PullRequestHosts::hosts(cx, None) == ["github.com"])
        });
        drop(task);
    }
}
