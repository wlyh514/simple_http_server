use std::collections::HashMap;

use num_enum::{IntoPrimitive, TryFromPrimitive};

use super::error::ErrorCode;

/// See RFC7540 section 6.5
#[repr(u16)]
#[derive(TryFromPrimitive, IntoPrimitive, Eq, PartialEq, Debug, Clone)]
pub enum SettingsIdentifier {
    HeaderTableSize = 0x1,
    EnablePush = 0x2,
    MaxConcurrentStreams = 0x3,
    InitialWindowSize = 0x4,
    MaxFrameSize = 0x5,
    MaxHeaderListSize = 0x6,
}
impl SettingsIdentifier {
    pub fn is_valid_value(&self, value: u32) -> Result<u32, ErrorCode> {
        const VALID_RANGES: [(SettingsIdentifier, u32, u32, ErrorCode); 3] = [
            (
                SettingsIdentifier::EnablePush,
                0,
                1,
                ErrorCode::ProtocolError,
            ),
            (
                SettingsIdentifier::InitialWindowSize,
                0,
                2147483647,
                ErrorCode::FlowControlError,
            ),
            (
                SettingsIdentifier::MaxFrameSize,
                16384,
                16777215,
                ErrorCode::ProtocolError,
            ),
        ];
        match VALID_RANGES.iter().find(|entry| entry.0 == *self) {
            Some((_identifier, min, max, error_code)) => {
                if min <= &value && &value <= max {
                    Ok(value)
                } else {
                    Err(error_code.clone())
                }
            }
            None => Ok(value),
        }
    }
}
#[derive(Clone)]
pub struct SettingsMap(HashMap<u16, u32>);
impl SettingsMap {
    /// Create a SettingsMap with default values
    pub fn default() -> Self {
        let mut settings_map = Self(HashMap::new());

        // Theses fields should ALWAYS exist within the lifespan of a SettingsMap
        settings_map
            .set(SettingsIdentifier::HeaderTableSize, 4096)
            .unwrap();
        settings_map.set(SettingsIdentifier::EnablePush, 1).unwrap();
        settings_map
            .set(SettingsIdentifier::MaxConcurrentStreams, 128)
            .unwrap();
        settings_map
            .set(SettingsIdentifier::InitialWindowSize, 65535)
            .unwrap();
        settings_map
            .set(SettingsIdentifier::MaxFrameSize, 16384)
            .unwrap();
        settings_map
            .set(SettingsIdentifier::MaxHeaderListSize, u32::MAX)
            .unwrap();

        settings_map
    }

    pub fn set(&mut self, identifier: SettingsIdentifier, value: u32) -> Result<(), ErrorCode> {
        self.0
            .insert(identifier.clone() as u16, identifier.is_valid_value(value)?);
        Ok(())
    }

    pub fn get(&self, identifier: SettingsIdentifier) -> Option<u32> {
        self.0.get(&(identifier as u16)).map(|val| *val)
    }

    pub fn update(&mut self, other: Self) {
        for (key, val) in other.0 {
            self.0.insert(key, val);
        }
    }

    pub fn update_with_vec(&mut self, other: &Vec<SettingParam>) -> Result<(), ErrorCode> {
        for setting in other {
            self.set(setting.identifier.clone(), setting.value)?;
        }
        Ok(())
    }
}
impl From<Vec<SettingParam>> for SettingsMap {
    fn from(params: Vec<SettingParam>) -> Self {
        let mut settings: HashMap<u16, u32> = HashMap::new();
        for param in params {
            settings.insert(param.identifier as u16, param.value);
        }
        Self(settings)
    }
}
impl Into<Vec<SettingParam>> for SettingsMap {
    fn into(self) -> Vec<SettingParam> {
        let mut params: Vec<SettingParam> = vec![];
        for (key, value) in self.0 {
            params.push(SettingParam {
                identifier: key.try_into().unwrap(),
                value,
            });
        }
        params
    }
}

#[derive(Clone, Debug)]
pub struct SettingParam {
    pub identifier: SettingsIdentifier,
    pub value: u32,
}
